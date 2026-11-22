#!/usr/bin/env python3
"""model-router: 给子智能体按角色选模型，并把"选了什么"落成回执。

    python tools/model_router.py route --role docs --require long_context --json
    python tools/model_router.py probe --strict
    python tools/model_router.py call --role docs --prompt-file task.md --out answer.md \
        --receipt .agent/model-receipt.json

设计口径（与 AGENT-TASK-STANDARD 的"可判真假"一致）：
  * 配置只写环境变量名，不写密钥字面量；`validate` 会拒绝 `api_key/token/secret` 字段
    与 `sk-...` 形状的值。配置本体建议放 `.agent/models.json`（本机，gitignore）。
  * `route` 只做可用性探测（命令在不在、环境变量在不在），**不发网络请求**；
    "可用"不等于"质量好"，质量必须靠评测/回执，不靠路由器的自称。
  * 找不到可用模型时 exit 2，不静默回退到某个付费模型；`--allow-fallback` 才允许
    显式退到 default 路由（仍要可用）。
  * `call` 支持两种 provider：`openai-chat`（OpenAI 兼容 /chat/completions）与
    `command`（本机 CLI，stdin 或 {prompt}/{prompt_file} 占位）。token 用量取 API
    usage；CLI 没有 usage 就记 null，不拿字符数冒充 token。

诚实边界：
  * 不是模型质量评测器；路由顺序是人的策略，不是 benchmark。
  * `command` provider 继承当前环境（除密钥类变量不清洗）；它适合受信任的本机 CLI，
    不适合跑不可信提示词。不可信执行请走 `tools/sandbox_run.py`。
  * HTTP 只做 scheme/host 形状校验，不做企业级 SSRF 防护；base_url 由配置者负责。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOOLS = os.path.dirname(os.path.abspath(__file__))
DEFAULT_CONFIG = os.path.join(os.path.dirname(TOOLS), ".agent", "models.json")
VERSION = 1
PROVIDERS = {"openai-chat", "command"}
COST_RANK = {"free": 0, "cheap": 1, "paid": 2, "unknown": 3}
ROLE_RX = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")
ID_RX = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
ENV_RX = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
SECRET_KEY_RX = re.compile(r"(api[_-]?key|token|secret|password|credential|authorization)", re.I)
SECRET_VALUE_RX = re.compile(
    r"(sk-[A-Za-z0-9_-]{16,}|AIza[0-9A-Za-z_-]{35}|ghp_[A-Za-z0-9]{20,}|"
    r"xox[baprs]-[A-Za-z0-9-]{10,})"
)


class ConfigError(Exception):
    pass


def load_config(path):
    try:
        with open(path, "r", encoding="utf-8-sig") as fh:
            cfg = json.load(fh)
    except OSError as exc:
        raise ConfigError(f"config unreadable: {exc}")
    except ValueError as exc:
        raise ConfigError(f"config is not valid JSON: {exc}")
    if not isinstance(cfg, dict):
        raise ConfigError("config root must be an object")
    return cfg


def _walk_keys(obj, path="config"):
    if isinstance(obj, dict):
        for key, value in obj.items():
            yield path + "." + str(key), str(key), value
            yield from _walk_keys(value, path + "." + str(key))
    elif isinstance(obj, list):
        for i, value in enumerate(obj):
            yield from _walk_keys(value, f"{path}[{i}]")


def validate(cfg):
    problems = []
    if cfg.get("version") != VERSION:
        problems.append(f"version must be {VERSION}")
    models = cfg.get("models")
    if not isinstance(models, list) or not models:
        problems.append("models must be a non-empty list")
        models = []
    routes = cfg.get("routes")
    if not isinstance(routes, dict) or not routes:
        problems.append("routes must be a non-empty object")
        routes = {}
    seen = set()
    valid_models = {}
    for i, model in enumerate(models):
        where = f"models[{i}]"
        if not isinstance(model, dict):
            problems.append(f"{where} must be an object")
            continue
        mid = model.get("id")
        if not isinstance(mid, str) or not ID_RX.match(mid):
            problems.append(f"{where}.id must match {ID_RX.pattern}")
            continue
        if mid in seen:
            problems.append(f"{where}.id duplicate: {mid}")
        seen.add(mid)
        valid_models[mid] = model
        provider = model.get("provider")
        if provider not in PROVIDERS:
            problems.append(f"{where}.provider must be one of {sorted(PROVIDERS)}")
        roles = model.get("roles")
        if not isinstance(roles, list) or not roles or not all(
                isinstance(x, str) and ROLE_RX.match(x) for x in roles):
            problems.append(f"{where}.roles must be a non-empty list of role names")
        priority = model.get("priority", 0)
        if not isinstance(priority, int) or isinstance(priority, bool) or priority < 0:
            problems.append(f"{where}.priority must be a non-negative integer")
        context = model.get("context", 0)
        if not isinstance(context, int) or isinstance(context, bool) or context < 0:
            problems.append(f"{where}.context must be a non-negative integer")
        cost = model.get("cost", "unknown")
        if cost not in COST_RANK:
            problems.append(f"{where}.cost must be one of {sorted(COST_RANK)}")
        caps = model.get("capabilities", [])
        if not isinstance(caps, list) or not all(isinstance(x, str) for x in caps):
            problems.append(f"{where}.capabilities must be a list of strings")
        if not isinstance(model.get("enabled", True), bool):
            problems.append(f"{where}.enabled must be a boolean")
        if provider == "openai-chat":
            for key in ("model", "base_url", "api_key_env"):
                if not isinstance(model.get(key), str) or not model[key].strip():
                    problems.append(f"{where}.{key} is required for openai-chat")
            env = model.get("api_key_env", "")
            if isinstance(env, str) and env and not ENV_RX.match(env):
                problems.append(f"{where}.api_key_env must be an environment variable name")
            url = model.get("base_url", "")
            if isinstance(url, str) and url:
                try:
                    parts = urllib.parse.urlsplit(url)
                except ValueError:
                    parts = None
                if (parts is None or parts.scheme not in ("http", "https")
                        or not parts.netloc or parts.username or parts.password):
                    problems.append(f"{where}.base_url must be a plain http(s) URL")
        if provider == "command":
            cmd = model.get("command")
            if not isinstance(cmd, list) or not cmd or not all(
                    isinstance(x, str) and x for x in cmd):
                problems.append(f"{where}.command must be a non-empty list of strings")
    for where, key, value in _walk_keys(cfg):
        if SECRET_KEY_RX.search(key) and key != "api_key_env":
            # api_key_env is the only sanctioned secret-shaped key: it names an env var.
            problems.append(f"{where}: secret-shaped key is not allowed (use api_key_env)")
        if isinstance(value, str) and SECRET_VALUE_RX.search(value):
            problems.append(f"{where}: secret-shaped literal is not allowed in config")
    for role, ids in routes.items():
        if not isinstance(role, str) or not (role == "default" or ROLE_RX.match(role)):
            problems.append(f"routes.{role}: bad role name")
        if not isinstance(ids, list) or not ids:
            problems.append(f"routes.{role}: must be a non-empty list of model ids")
            continue
        for mid in ids:
            if mid not in valid_models:
                problems.append(f"routes.{role}: unknown model {mid!r}")
        if len(ids) != len(set(ids)):
            problems.append(f"routes.{role}: duplicate model ids")
    return problems


def model_available(model, getenv=os.environ.get):
    if not model.get("enabled", True):
        return False, "disabled"
    provider = model.get("provider")
    if provider == "command":
        first = model["command"][0]
        if os.path.isabs(first):
            found = os.path.exists(first)
        else:
            found = shutil.which(first) is not None
        return (True, "") if found else (False, f"command not found: {first}")
    if provider == "openai-chat":
        env = model.get("api_key_env", "")
        if not env or not getenv(env):
            return False, f"environment variable not set: {env or '<missing api_key_env>'}"
        return True, ""
    return False, f"unknown provider: {provider}"


def _models_by_id(cfg):
    return {m["id"]: m for m in cfg.get("models", []) if isinstance(m, dict) and "id" in m}


def choose(cfg, role, require=(), prefer=""):
    """Return (model, why, candidates). candidates include the unavailable reasons."""
    by_id = _models_by_id(cfg)
    route_ids = cfg.get("routes", {}).get(role) or cfg.get("routes", {}).get("default") or []
    required = {x for x in require if x}
    candidates = []
    available = []
    for mid in route_ids:
        model = by_id.get(mid)
        if not model:
            candidates.append({"id": mid, "available": False, "reason": "unknown model id"})
            continue
        caps = set(model.get("capabilities", []))
        if required - caps:
            candidates.append({"id": mid, "available": False,
                               "reason": "missing capabilities: " + ",".join(sorted(required - caps))})
            continue
        ok, reason = model_available(model)
        candidates.append({"id": mid, "available": ok, "reason": reason})
        if ok:
            available.append(model)
    if not available:
        return None, f"no available model for role={role}", candidates
    if prefer:
        if prefer == "priority":
            available.sort(key=lambda m: (-int(m.get("priority", 0)), m["id"]))
        elif prefer == "cost":
            available.sort(key=lambda m: (COST_RANK.get(m.get("cost", "unknown"), 3),
                                          -int(m.get("priority", 0)), m["id"]))
        elif prefer == "context":
            available.sort(key=lambda m: (-int(m.get("context", 0)),
                                          -int(m.get("priority", 0)), m["id"]))
        else:
            return None, f"unknown --prefer {prefer!r}", candidates
    model = available[0]
    why = (f"role={role} route={','.join(route_ids)} -> {model['id']}"
           + (f"; prefer={prefer}" if prefer else ""))
    return model, why, candidates


def cmd_validate(cfg, args):
    problems = validate(cfg)
    if args.json:
        print(json.dumps({"tool": "model-router", "ok": not problems,
                          "problems": problems}, ensure_ascii=True))
    else:
        for problem in problems:
            print("model-router: " + problem)
        print("model-router: " + ("OK config" if not problems else f"FAIL {len(problems)} problem(s)"))
    return 0 if not problems else 1


def cmd_probe(cfg, args):
    rows = []
    for model in cfg.get("models", []):
        if not isinstance(model, dict):
            continue
        ok, reason = model_available(model)
        rows.append({"id": model.get("id"), "provider": model.get("provider"),
                     "available": ok, "reason": reason,
                     "evidence": ("env-present" if ok and model.get("provider") == "openai-chat"
                                  else "command-found" if ok else "unavailable"),
                     "roles": model.get("roles", [])})
    enabled = [r for r in rows if r["available"]]
    strict_ok = all(r["available"] for r in rows)
    ok = bool(enabled) and (strict_ok if args.strict else True)
    if args.json:
        print(json.dumps({"tool": "model-router", "ok": ok, "models": rows},
                         ensure_ascii=True))
    else:
        for row in rows:
            print(("env* " if row["available"] else "MISS ") + f"{row['id']} ({row['provider']})"
                  + (f" -- {row['reason']}" if not row["available"]
                     else f" -- {row['evidence']}"))
        print("model-router: probe 只证明命令/环境变量在场，不证明鉴权、额度、网络或模型名；"
              "真实调用看 `call` 的 exit/回执")
        print("model-router: " + ("OK at least one available" if ok else "FAIL no usable model"))
    return 0 if ok else 1


def cmd_route(cfg, args):
    model, why, candidates = choose(cfg, args.role, args.require, args.prefer)
    if model is None:
        if args.json:
            print(json.dumps({"tool": "model-router", "role": args.role, "model": None,
                              "why": why, "candidates": candidates}, ensure_ascii=True))
        else:
            print("model-router: " + why)
            for row in candidates:
                print(f"  - {row['id']}: {'ok' if row['available'] else row['reason']}")
        return 2
    if args.json:
        print(json.dumps({"tool": "model-router", "role": args.role,
                          "model": model["id"], "provider": model.get("provider"),
                          "provider_model": model.get("model", ""),
                          "why": why, "candidates": candidates}, ensure_ascii=True))
    else:
        print(f"{args.role} -> {model['id']} ({model.get('provider')})  [{why}]")
    return 0


def _sha256(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _write_json(path, doc):
    if not path:
        return
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(doc, fh, ensure_ascii=True, indent=2)
        fh.write("\n")


def _http_chat(model, prompt, max_tokens, temperature, timeout):
    base = model["base_url"].rstrip("/")
    url = base if base.endswith("/chat/completions") else base + "/chat/completions"
    body = {"model": model["model"], "messages": [{"role": "user", "content": prompt}]}
    if max_tokens:
        body["max_tokens"] = max_tokens
    if temperature is not None:
        body["temperature"] = temperature
    data = json.dumps(body, ensure_ascii=False).encode("utf-8")
    req = urllib.request.Request(url, data=data, method="POST", headers={
        "Content-Type": "application/json",
        "Authorization": "Bearer " + os.environ.get(model["api_key_env"], ""),
    })
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read(4 * 1024 * 1024)
            doc = json.loads(raw.decode("utf-8", errors="replace"))
    except urllib.error.HTTPError as exc:
        detail = exc.read(4096).decode("utf-8", errors="replace")
        raise RuntimeError(f"HTTP {exc.code}: {detail[:300]}") from exc
    except (urllib.error.URLError, TimeoutError, ValueError) as exc:
        raise RuntimeError(f"request failed: {exc}") from exc
    choices = doc.get("choices") if isinstance(doc, dict) else None
    if not isinstance(choices, list) or not choices:
        raise RuntimeError("response has no choices")
    message = choices[0].get("message") if isinstance(choices[0], dict) else None
    content = message.get("content") if isinstance(message, dict) else None
    if not isinstance(content, str) or not content:
        raise RuntimeError("response has no message.content")
    return {"output": content, "usage": doc.get("usage"),
            "response_id": doc.get("id"), "seconds": time.time() - t0}


def _command_chat(model, prompt, timeout):
    argv = list(model["command"])
    has_placeholder = any("{prompt}" in a or "{prompt_file}" in a for a in argv)
    prompt_file = None
    if any("{prompt_file}" in a for a in argv):
        fd, prompt_file = tempfile.mkstemp(prefix="model-prompt-", suffix=".txt")
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(prompt)
    try:
        try:
            argv = [a.format(prompt=prompt, prompt_file=prompt_file or "",
                             role=model.get("_role", ""), model=model.get("model", ""))
                    for a in argv]
        except (KeyError, IndexError, ValueError) as exc:
            raise RuntimeError(f"command template invalid: {exc}") from exc
        t0 = time.time()
        proc = subprocess.run(argv, input=None if has_placeholder else prompt,
                              capture_output=True, text=True, encoding="utf-8",
                              errors="replace", timeout=timeout)
        seconds = time.time() - t0
        if proc.returncode != 0:
            raise RuntimeError(f"command exit {proc.returncode}: {(proc.stderr or proc.stdout)[:300]}")
        if not proc.stdout.strip():
            raise RuntimeError("command produced empty stdout")
        return {"output": proc.stdout.rstrip("\n"), "usage": None,
                "response_id": None, "seconds": seconds}
    except subprocess.TimeoutExpired as exc:
        raise RuntimeError(f"command timeout after {timeout}s") from exc
    finally:
        if prompt_file:
            try:
                os.unlink(prompt_file)
            except OSError:
                pass


def cmd_call(cfg, args):
    if args.model:
        model = _models_by_id(cfg).get(args.model)
        if not model:
            print(f"model-router: unknown model {args.model!r}", file=sys.stderr)
            return 2
    else:
        model, why, candidates = choose(cfg, args.role, (), "")
        if model is None:
            print("model-router: " + why, file=sys.stderr)
            for row in candidates:
                print(f"  - {row['id']}: {'ok' if row['available'] else row['reason']}", file=sys.stderr)
            return 2
    ok, reason = model_available(model)
    if not ok:
        print(f"model-router: model {model['id']} unavailable: {reason}", file=sys.stderr)
        return 2
    prompt = args.prompt
    if args.prompt_file:
        try:
            with open(args.prompt_file, "r", encoding="utf-8-sig") as fh:
                prompt = fh.read()
        except OSError as exc:
            print(f"model-router: prompt file unreadable: {exc}", file=sys.stderr)
            return 2
    if not isinstance(prompt, str) or not prompt.strip():
        print("model-router: --prompt or --prompt-file must be non-empty", file=sys.stderr)
        return 2
    model["_role"] = args.role
    receipt = {"tool": "model-router", "role": args.role, "model": model["id"],
               "provider": model.get("provider"), "prompt_sha256": _sha256(prompt),
               "prompt_bytes": len(prompt.encode("utf-8")), "ok": False,
               "exit": 2, "seconds": 0, "usage": None, "output_sha256": None,
               "output_bytes": 0, "error": None}
    t0 = time.time()
    try:
        if model.get("provider") == "openai-chat":
            result = _http_chat(model, prompt, args.max_tokens, args.temperature, args.timeout)
        else:
            result = _command_chat(model, prompt, args.timeout)
    except RuntimeError as exc:
        receipt["error"] = str(exc)[:500]
        receipt["seconds"] = round(time.time() - t0, 3)
        _write_json(args.receipt, receipt)
        print(f"model-router: call failed: {exc}", file=sys.stderr)
        return 2
    output = result["output"]
    if args.out:
        os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
        with open(args.out, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(output)
            if not output.endswith("\n"):
                fh.write("\n")
    else:
        print(output)
    receipt.update({"ok": True, "exit": 0, "seconds": round(result["seconds"], 3),
                    "usage": result.get("usage"), "response_id": result.get("response_id"),
                    "output_sha256": _sha256(output),
                    "output_bytes": len(output.encode("utf-8"))})
    _write_json(args.receipt, receipt)
    if args.json:
        print(json.dumps({k: receipt[k] for k in ("tool", "role", "model", "provider",
                                                   "ok", "exit", "seconds", "usage",
                                                   "output_sha256", "output_bytes")},
                         ensure_ascii=True))
    return 0


def make_parser():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--config", default=DEFAULT_CONFIG)
    p.add_argument("--self-test", action="store_true")
    sub = p.add_subparsers(dest="cmd")

    v = sub.add_parser("validate")
    v.add_argument("--json", action="store_true")

    pr = sub.add_parser("probe")
    pr.add_argument("--strict", action="store_true")
    pr.add_argument("--json", action="store_true")

    r = sub.add_parser("route")
    r.add_argument("--role", default="default")
    r.add_argument("--require", action="append", default=[])
    r.add_argument("--prefer", default="", choices=["", "priority", "cost", "context"])
    r.add_argument("--json", action="store_true")

    c = sub.add_parser("call")
    c.add_argument("--role", default="default")
    c.add_argument("--model", default="")
    c.add_argument("--prompt", default="")
    c.add_argument("--prompt-file", default="")
    c.add_argument("--out", default="")
    c.add_argument("--receipt", default="")
    c.add_argument("--max-tokens", type=int, default=0)
    c.add_argument("--temperature", type=float, default=None)
    c.add_argument("--timeout", type=float, default=120.0)
    c.add_argument("--json", action="store_true")
    return p


def self_test():
    """Hermetic: local HTTP stub + local command adapter; no vendor call, no spend."""
    checks = []

    def ck(name, cond, detail=""):
        checks.append((name, bool(cond), detail))

    tmp = tempfile.mkdtemp(prefix="model-router-self-")
    try:
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                length = int(self.headers.get("Content-Length", "0"))
                body = self.rfile.read(length)
                if "/fail" in self.path:
                    self.send_response(500)
                    self.end_headers()
                    self.wfile.write(b'{"error":"stub fail"}')
                    return
                try:
                    doc = json.loads(body)
                    assert doc["messages"][0]["content"] == "hello"
                    assert self.headers.get("Authorization") == "Bearer test-key"
                except Exception:
                    self.send_response(400)
                    self.end_headers()
                    self.wfile.write(b'{"error":"bad request"}')
                    return
                payload = json.dumps({"id": "stub-1",
                                      "choices": [{"message": {"content": "HTTP:hello"}}],
                                      "usage": {"prompt_tokens": 3, "completion_tokens": 5}}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        srv = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=srv.serve_forever, daemon=True)
        thread.start()
        port = srv.server_address[1]
        os.environ["TRIT_MODEL_TEST_KEY"] = "test-key"
        cfg = {
            "version": 1,
            "models": [
                {"id": "cmd-docs", "provider": "command",
                 "command": [sys.executable, "-c",
                             "import sys; print('CMD:' + sys.stdin.read().strip())"],
                 "roles": ["docs"], "priority": 1, "cost": "free", "context": 1000,
                 "capabilities": ["text"]},
                {"id": "cmd-code", "provider": "command",
                 "command": [sys.executable, "-c", "print('CODE')"],
                 "roles": ["code"], "priority": 2, "cost": "free", "context": 1000,
                 "capabilities": ["text"]},
                {"id": "http-docs", "provider": "openai-chat",
                 "base_url": f"http://127.0.0.1:{port}/v1", "model": "stub-model",
                 "api_key_env": "TRIT_MODEL_TEST_KEY", "roles": ["docs"],
                 "priority": 8, "cost": "paid", "context": 32000,
                 "capabilities": ["text", "long_context"]},
            ],
            "routes": {"docs": ["http-docs", "cmd-docs"], "code": ["cmd-code"],
                       "default": ["cmd-code"]},
        }
        cfg_path = os.path.join(tmp, "models.json")
        with open(cfg_path, "w", encoding="utf-8", newline="\n") as fh:
            json.dump(cfg, fh)
        ck("validate good config", validate(cfg) == [], str(validate(cfg)))

        model, why, cand = choose(cfg, "docs")
        ck("route docs prefers available http model", model and model["id"] == "http-docs", why)
        model_cost, _, _ = choose(cfg, "docs", (), "cost")
        ck("prefer cost picks free local model", model_cost and model_cost["id"] == "cmd-docs",
           model_cost["id"] if model_cost else "")
        model_code, _, _ = choose(cfg, "code")
        ck("route code", model_code and model_code["id"] == "cmd-code", "")
        missing, why2, cand2 = choose(cfg, "unknown")
        ck("unknown role falls back to default", missing and missing["id"] == "cmd-code", why2)
        vis, why3, cand3 = choose(cfg, "docs", ("vision",))
        ck("missing capability refuses", vis is None, why3)

        bad = json.loads(json.dumps(cfg))
        bad["models"].append(dict(bad["models"][0]))
        ck("duplicate id rejected", any("duplicate" in p for p in validate(bad)))
        bad2 = json.loads(json.dumps(cfg))
        bad2["routes"]["docs"] = ["gone"]
        ck("dangling route rejected", any("unknown model" in p for p in validate(bad2)))
        bad3 = json.loads(json.dumps(cfg))
        bad3["models"][0]["api_key"] = "sk-" + "a" * 20
        ck("literal secret rejected", any("secret" in p for p in validate(bad3)))
        bad4 = json.loads(json.dumps(cfg))
        bad4["models"][2]["base_url"] = "file:///etc/passwd"
        ck("non-http base_url rejected", any("base_url" in p for p in validate(bad4)))

        p = make_parser()
        route_args = p.parse_args(["--config", cfg_path, "route", "--role", "docs", "--json"])
        saved = sys.stdout
        out = __import__("io").StringIO()
        sys.stdout = out
        try:
            rc = cmd_route(cfg, route_args)
        finally:
            sys.stdout = saved
        ck("route --json", rc == 0 and json.loads(out.getvalue())["model"] == "http-docs",
           str(rc))

        call_args = p.parse_args(["--config", cfg_path, "call", "--role", "code",
                                  "--prompt", "hello", "--receipt",
                                  os.path.join(tmp, "r-command.json")])
        saved = sys.stdout
        out = __import__("io").StringIO()
        sys.stdout = out
        try:
            rc = cmd_call(cfg, call_args)
        finally:
            sys.stdout = saved
        ck("command provider call", rc == 0 and "CODE" in out.getvalue(), str(rc))

        call_args = p.parse_args(["--config", cfg_path, "call", "--role", "docs",
                                  "--prompt", "hello", "--out",
                                  os.path.join(tmp, "answer.txt"), "--receipt",
                                  os.path.join(tmp, "r-http.json"), "--json"])
        saved = sys.stdout
        out = __import__("io").StringIO()
        sys.stdout = out
        try:
            rc = cmd_call(cfg, call_args)
        finally:
            sys.stdout = saved
        receipt = json.load(open(os.path.join(tmp, "r-http.json"), encoding="utf-8"))
        answer = open(os.path.join(tmp, "answer.txt"), encoding="utf-8").read().strip()
        ck("http provider call + usage", rc == 0 and answer == "HTTP:hello"
           and receipt["usage"]["completion_tokens"] == 5, str(rc))

        fail_cfg = json.loads(json.dumps(cfg))
        fail_cfg["models"][2]["base_url"] = f"http://127.0.0.1:{port}/v1/fail"
        fail_args = p.parse_args(["--config", cfg_path, "call", "--model", "http-docs",
                                  "--prompt", "hello", "--receipt",
                                  os.path.join(tmp, "r-fail.json")])
        # use a config where the http model points at /fail
        saved = sys.stdout
        sys.stdout = __import__("io").StringIO()
        try:
            rc = cmd_call(fail_cfg, fail_args)
        finally:
            sys.stdout = saved
        fail_receipt = json.load(open(os.path.join(tmp, "r-fail.json"), encoding="utf-8"))
        ck("http failure recorded, no silent success", rc == 2 and not fail_receipt["ok"],
           str(rc))

        timeout_args = p.parse_args(["--config", cfg_path, "call", "--role", "code",
                                     "--prompt", "hello", "--timeout", "1"])
        timeout_cfg = json.loads(json.dumps(cfg))
        timeout_cfg["models"][1]["command"] = [sys.executable, "-c", "import time; time.sleep(3)"]
        saved = sys.stdout
        sys.stdout = __import__("io").StringIO()
        try:
            rc = cmd_call(timeout_cfg, timeout_args)
        finally:
            sys.stdout = saved
        ck("command timeout refuses", rc == 2, str(rc))

        probe_args = p.parse_args(["--config", cfg_path, "probe", "--json"])
        saved = sys.stdout
        out = __import__("io").StringIO()
        sys.stdout = out
        try:
            rc = cmd_probe(cfg, probe_args)
        finally:
            sys.stdout = saved
        ck("probe reports availability", rc == 0 and all(
            x["available"] for x in json.loads(out.getvalue())["models"]), str(rc))

        srv.shutdown()
        srv.server_close()
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        os.environ.pop("TRIT_MODEL_TEST_KEY", None)

    bad = [c for c in checks if not c[1]]
    for name, ok, detail in checks:
        print(("ok   " if ok else "FAIL ") + name + (f"  [{detail}]" if detail and not ok else ""))
    print(f"model-router self-test: {len(checks) - len(bad)}/{len(checks)}")
    return 0 if not bad else 1


def main(argv):
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    p = make_parser()
    args = p.parse_args(argv)
    if args.self_test:
        return self_test()
    if not args.cmd:
        p.print_help()
        return 2
    try:
        cfg = load_config(args.config)
    except ConfigError as exc:
        print(f"model-router: {exc}", file=sys.stderr)
        return 2
    problems = validate(cfg)
    if problems and args.cmd != "validate":
        print("model-router: config invalid (run `validate`)", file=sys.stderr)
        for problem in problems[:8]:
            print("  - " + problem, file=sys.stderr)
        return 2
    if args.cmd == "validate":
        return cmd_validate(cfg, args)
    if args.cmd == "probe":
        return cmd_probe(cfg, args)
    if args.cmd == "route":
        return cmd_route(cfg, args)
    if args.cmd == "call":
        return cmd_call(cfg, args)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
