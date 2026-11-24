#!/usr/bin/env python3
"""stability-probe: 重复执行命令，区分“稳定通过”和“抖动”。

一次通过不等于稳定：同一个测试可能这次过、下次挂；同一个命令的 stdout 可能每次
不同；同一批命令的耗时会突然抖一下。本工具把“抖动”拆成可复算的观测：

  - stable-pass:   所有重复运行都 exit 0，输出指纹一致，耗时波动在阈值内；
  - stable-fail:   所有重复运行都非 0，输出指纹一致；这是确定性失败，不是 flaky；
  - flaky:         同一命令既有 exit 0 又有非 0（最常见的 flaky test）；
  - nondeterministic: 都 exit 0，但输出指纹在重复运行间不同（要求输出一致时）；
  - jittery:       都 exit 0、输出一致，但 max/min 耗时超过阈值；
  - unknown:       超时、启动失败或无法判定；unknown 不是稳定，按 fail-closed。

预热：`warmup` 先跑 N 次，这几次的退出码参与 flaky 判定（预热就挂不能算稳定），
但耗时和输出指纹只统计后面的 `repeat` 次，避免把首次编译/冷启动误判成 jitter。

清单 `stability/1` 示例：

```json
{
  "schema": "stability/1",
  "repeat": 3,
  "warmup": 1,
  "timeout_s": 120,
  "max_duration_ratio": 3.0,
  "require_identical_output": true,
  "commands": [
    {"id": "T1", "command": "python -m pytest tests/test_a.py -q", "repeat": 5}
  ]
}
```

用法：

  python tools/stability_probe.py --root . --manifest .agent/stability.json
  python tools/stability_probe.py --root . --manifest .agent/stability.json --json
  python tools/stability_probe.py --root . --manifest .agent/stability.json \
      --repeat 5 --timeout-s 60 --out target/stability.json
  python tools/stability_probe.py self-test

退出码：0 全部 stable-pass；1 有 flaky/nondeterministic/jittery/确定性失败；
2 坏调用；3 unknown（清单缺失/读不出，或命令超时/无法启动）。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone

SCHEMA = "stability/1"
TOP_FIELDS = {"schema", "repeat", "warmup", "timeout_s", "max_duration_ratio",
              "require_identical_output", "commands", "notes"}
CMD_FIELDS = {"id", "command", "repeat", "warmup", "timeout_s", "max_duration_ratio",
              "require_identical_output", "notes"}
REPEAT_LOW, REPEAT_HIGH = 2, 50
WARMUP_LOW, WARMUP_HIGH = 0, 5
TIMEOUT_LOW, TIMEOUT_HIGH = 0.1, 3600.0
RATIO_LOW, RATIO_HIGH = 1.0, 100.0
UNSTABLE = {"flaky", "nondeterministic", "jittery", "unstable-fail"}
FAILED = {"stable-fail", "unstable-fail"}


def _now():
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def _display(root, path):
    try:
        rel = os.path.relpath(os.path.abspath(path), os.path.abspath(root))
    except ValueError:
        return os.path.basename(path)
    if rel == ".":
        return "."
    if rel == os.pardir or rel.startswith(os.pardir + os.sep):
        return os.path.basename(path)
    return rel.replace("\\", "/")


def _build_report(root, manifest):
    return {
        "schema": SCHEMA,
        "status": "unknown",
        "exit": 3,
        "manifest": _display(root, manifest),
        "generated_at": _now(),
        "config": {"repeat": 3, "warmup": 0, "timeout_s": 120.0,
                   "max_duration_ratio": 3.0, "require_identical_output": True},
        "counts": {"commands": 0, "runs": 0, "warmup_runs": 0,
                   "stable_pass": 0, "stable_fail": 0,
                   "flaky": 0, "nondeterministic": 0, "jittery": 0,
                   "unstable_fail": 0, "unknown": 0},
        "commands": [],
        "failures": [],
    }


def _fail(report, code, message, node=None):
    report["failures"].append({"code": code, "message": message,
                               **({"node": node} if node else {})})


def _unknown_fields(obj, allowed, node, report):
    for key in obj:
        if key not in allowed:
            _fail(report, "unknown-field", f"{node} 不接受未建模字段：{key}", node)


def _get_int(obj, key, default, low, high, node, report):
    value = obj.get(key, default)
    if isinstance(value, bool) or not isinstance(value, int) or not (low <= value <= high):
        _fail(report, "bad-int", f"{node}.{key} 必须是 {low}..{high} 的整数：{value!r}", node)
        return low
    return value


def _get_float(obj, key, default, low, high, node, report):
    value = obj.get(key, default)
    if isinstance(value, bool) or not isinstance(value, (int, float)) \
            or not (low <= float(value) <= high):
        _fail(report, "bad-number",
              f"{node}.{key} 必须是 {low}..{high} 的数：{value!r}", node)
        return low
    return float(value)


def _get_bool(obj, key, default, node, report):
    value = obj.get(key, default)
    if not isinstance(value, bool):
        _fail(report, "bad-bool", f"{node}.{key} 必须是布尔值：{value!r}", node)
        return bool(default)
    return value


def load_manifest(root, manifest_path):
    """Read and validate a stability/1 manifest. Unknown is not a pass."""
    root = os.path.abspath(root)
    manifest_path = os.path.abspath(manifest_path)
    report = _build_report(root, manifest_path)
    root_real = os.path.realpath(root)
    manifest_real = os.path.realpath(manifest_path)
    try:
        common = os.path.commonpath([root_real, manifest_real])
    except ValueError:
        common = ""
    if os.path.normcase(common) != os.path.normcase(root_real):
        _fail(report, "manifest-outside-root",
              f"清单路径必须留在项目 root 内：{report['manifest']}")
        return None, report
    if not os.path.isfile(manifest_real):
        _fail(report, "manifest-missing", f"稳定性清单不存在：{report['manifest']}")
        return None, report
    try:
        with open(manifest_real, "rb") as fh:
            doc = json.loads(fh.read().decode("utf-8-sig"))
    except (OSError, UnicodeDecodeError, ValueError) as e:
        _fail(report, "manifest-unreadable", f"稳定性清单读不出或坏 JSON：{e}")
        return None, report
    if not isinstance(doc, dict):
        _fail(report, "manifest-unreadable", "稳定性清单必须是 JSON 对象")
        return None, report
    if doc.get("schema") != SCHEMA:
        _fail(report, "schema-mismatch", f"schema 必须是 {SCHEMA!r}，实际 {doc.get('schema')!r}")
        return None, report
    _unknown_fields(doc, TOP_FIELDS, "manifest", report)

    repeat = _get_int(doc, "repeat", 3, REPEAT_LOW, REPEAT_HIGH, "manifest", report)
    warmup = _get_int(doc, "warmup", 0, WARMUP_LOW, WARMUP_HIGH, "manifest", report)
    timeout_s = _get_float(doc, "timeout_s", 120.0, TIMEOUT_LOW, TIMEOUT_HIGH,
                           "manifest", report)
    ratio = _get_float(doc, "max_duration_ratio", 3.0, RATIO_LOW, RATIO_HIGH,
                       "manifest", report)
    identical = _get_bool(doc, "require_identical_output", True, "manifest", report)
    commands = doc.get("commands")
    if not isinstance(commands, list) or not commands:
        _fail(report, "commands-missing", "commands 必须是非空数组", "commands")
        commands = []
    ids = set()
    normalized = []
    for i, cmd in enumerate(commands):
        node = f"commands[{i}]"
        if not isinstance(cmd, dict):
            _fail(report, "command-invalid", f"{node} 必须是对象", node)
            continue
        _unknown_fields(cmd, CMD_FIELDS, node, report)
        cid = cmd.get("id")
        if not isinstance(cid, str) or not cid.strip() or any(ch in cid for ch in "\r\n\t"):
            _fail(report, "command-id", f"{node}.id 必须是非空单行字符串", node)
            continue
        cid = cid.strip()
        if cid in ids:
            _fail(report, "duplicate-id", f"id 重复：{cid}", cid)
            continue
        ids.add(cid)
        command = cmd.get("command")
        if not isinstance(command, str) or not command.strip():
            _fail(report, "command-missing", f"{node}.command 必须是非空命令", cid)
            continue
        normalized.append({
            "id": cid,
            "command": command,
            "repeat": _get_int(cmd, "repeat", repeat, REPEAT_LOW, REPEAT_HIGH, cid, report),
            "warmup": _get_int(cmd, "warmup", warmup, WARMUP_LOW, WARMUP_HIGH,
                               cid, report),
            "timeout_s": _get_float(cmd, "timeout_s", timeout_s, TIMEOUT_LOW,
                                    TIMEOUT_HIGH, cid, report),
            "max_duration_ratio": _get_float(cmd, "max_duration_ratio", ratio,
                                             RATIO_LOW, RATIO_HIGH, cid, report),
            "require_identical_output": _get_bool(
                cmd, "require_identical_output", identical, cid, report),
        })
    report["config"] = {"repeat": repeat, "warmup": warmup, "timeout_s": timeout_s,
                        "max_duration_ratio": ratio,
                        "require_identical_output": identical}
    report["counts"]["commands"] = len(normalized)
    if report["failures"]:
        return None, report
    return normalized, report


def _digest_file(fh):
    fh.flush()
    fh.seek(0)
    h = hashlib.sha256()
    total = 0
    while True:
        chunk = fh.read(65536)
        if not chunk:
            break
        total += len(chunk)
        h.update(chunk)
    return h.hexdigest(), total


def _kill_tree(proc):
    """Kill the shell and its children; a timed-out child must not survive."""
    if os.name == "nt":
        try:
            subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           check=False)
        except OSError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        return
    try:
        # SIGKILL is 9 on POSIX. Do not `import signal`: tools/signal.py in this
        # repo shadows the stdlib module when this script is launched from tools/.
        os.killpg(os.getpgid(proc.pid), 9)
    except (ProcessLookupError, PermissionError, OSError):
        proc.kill()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()


def run_once(root, command, timeout_s):
    """Run one command once, hashing its output instead of storing it."""
    t0 = time.perf_counter()
    timed_out = False
    error = None
    code = None
    output_sha = None
    output_bytes = 0
    with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
        kwargs = {
            "shell": True,
            "cwd": root,
            "stdout": out,
            "stderr": err,
        }
        if os.name == "nt":
            kwargs["creationflags"] = subprocess.CREATE_NEW_PROCESS_GROUP
        else:
            kwargs["start_new_session"] = True
        try:
            proc = subprocess.Popen(command, **kwargs)
        except OSError as e:
            error = str(e)
            proc = None
        if proc is not None:
            try:
                proc.communicate(timeout=float(timeout_s))
                code = proc.returncode
            except subprocess.TimeoutExpired:
                timed_out = True
                _kill_tree(proc)
            except OSError as e:
                error = str(e)
                _kill_tree(proc)
        seconds = round(time.perf_counter() - t0, 6)
        stdout_sha, stdout_bytes = _digest_file(out)
        stderr_sha, stderr_bytes = _digest_file(err)
        output_sha = hashlib.sha256(
            f"{stdout_sha}\0{stderr_sha}".encode("ascii")).hexdigest()
        output_bytes = stdout_bytes + stderr_bytes
    return {"exit": code, "timeout": timed_out, "error": error,
            "seconds": seconds, "output_sha": output_sha,
            "output_bytes": output_bytes}


def _classify(warmup_runs, runs, identical, max_ratio):
    """Warmup exits count as flakiness; their durations/outputs do not measure."""
    all_runs = warmup_runs + runs
    if any(r["timeout"] or r["error"] for r in all_runs):
        return "unknown"
    exits = [r["exit"] for r in all_runs]
    passed = sum(1 for x in exits if x == 0)
    distinct_exits = len(set(exits))
    distinct_outputs = len({r["output_sha"] for r in runs})
    durations = sorted(float(r["seconds"]) for r in runs)
    dmin = durations[0]
    dmax = durations[-1]
    duration_ratio = (dmax / dmin) if dmin > 0 else None
    if 0 < passed < len(exits):
        return "flaky"
    if passed == 0:
        if distinct_exits > 1 or (identical and distinct_outputs > 1):
            return "unstable-fail"
        return "stable-fail"
    if identical and distinct_outputs > 1:
        return "nondeterministic"
    if duration_ratio is not None and dmin >= 0.001 and duration_ratio > max_ratio:
        return "jittery"
    return "stable-pass"


def _probe_command(root, spec):
    warmup_runs = [
        run_once(root, spec["command"], spec["timeout_s"])
        for _ in range(spec["warmup"])
    ]
    runs = []
    for _ in range(spec["repeat"]):
        runs.append(run_once(root, spec["command"], spec["timeout_s"]))
    classification = _classify(warmup_runs, runs,
                              spec["require_identical_output"],
                              spec["max_duration_ratio"])
    exits = [r["exit"] for r in runs]
    durations = [round(float(r["seconds"]) * 1000, 3) for r in runs]
    dmin = min(float(r["seconds"]) for r in runs)
    dmax = max(float(r["seconds"]) for r in runs)
    duration_ratio = round(dmax / dmin, 3) if dmin > 0 else None
    warmup_exits = [r["exit"] for r in warmup_runs]
    return {
        "id": spec["id"],
        "command": spec["command"],
        "repeat": spec["repeat"],
        "warmup": spec["warmup"],
        "timeout_s": spec["timeout_s"],
        "max_duration_ratio": spec["max_duration_ratio"],
        "require_identical_output": spec["require_identical_output"],
        "classification": classification,
        "exit_codes": exits,
        "warmup_exit_codes": warmup_exits,
        "passes": sum(1 for x in exits if x == 0),
        "failures": sum(1 for x in exits if x != 0),
        "warmup_failures": sum(1 for x in warmup_exits if x != 0),
        "distinct_outputs": len({r["output_sha"] for r in runs}),
        "duration_ms": durations,
        "duration_min_ms": round(dmin * 1000, 3),
        "duration_max_ms": round(dmax * 1000, 3),
        "duration_ratio": duration_ratio,
        "warmup_runs": warmup_runs,
        "runs": runs,
    }


def probe(root, manifest, repeat=None, timeout_s=None, ratio=None,
          allow_output_drift=False, warmup=None):
    """Run the manifest and return an auditable report."""
    root = os.path.abspath(root)
    commands, report = load_manifest(root, manifest)
    if commands is None:
        report["failures"] = report["failures"] or [
            {"code": "unknown", "message": "稳定性清单不可读"}]
        return report
    if repeat is not None:
        for spec in commands:
            spec["repeat"] = repeat
    if timeout_s is not None:
        for spec in commands:
            spec["timeout_s"] = timeout_s
    if ratio is not None:
        for spec in commands:
            spec["max_duration_ratio"] = ratio
    if allow_output_drift:
        for spec in commands:
            spec["require_identical_output"] = False
    if warmup is not None:
        for spec in commands:
            spec["warmup"] = warmup
    for spec in commands:
        result = _probe_command(root, spec)
        report["commands"].append(result)
        report["counts"]["runs"] += spec["repeat"] + spec["warmup"]
        report["counts"]["warmup_runs"] += spec["warmup"]
        report["counts"][result["classification"].replace("-", "_")] = (
            report["counts"].get(result["classification"].replace("-", "_"), 0) + 1)
        if result["classification"] in UNSTABLE or result["classification"] in FAILED:
            _fail(report, result["classification"],
                  f"{spec['id']} 分类为 {result['classification']}："
                  f"passes={result['passes']}/{spec['repeat']}，"
                  f"warmup_failures={result['warmup_failures']}/{spec['warmup']}，"
                  f"outputs={result['distinct_outputs']}，"
                  f"duration_ratio={result['duration_ratio']}", spec["id"])
        elif result["classification"] == "unknown":
            _fail(report, "probe-unknown",
                  f"{spec['id']} 无法判定（超时或启动失败）", spec["id"])

    classes = {c["classification"] for c in report["commands"]}
    if "unknown" in classes:
        report["status"], report["exit"] = "unknown", 3
    elif classes & UNSTABLE:
        report["status"], report["exit"] = "unstable", 1
    elif classes & {"stable-fail", "unstable-fail"}:
        report["status"], report["exit"] = "failed", 1
    else:
        report["status"], report["exit"] = "stable", 0
    return report


def _emit(report, as_json, out_path):
    if out_path:
        os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)
        with open(out_path, "w", encoding="ascii", newline="\n") as fh:
            json.dump(report, fh, ensure_ascii=True, sort_keys=True)
            fh.write("\n")
    if as_json:
        print(json.dumps(report, ensure_ascii=True, sort_keys=True))
        return
    c = report["counts"]
    print(f"stability: {report['status']} (exit={report['exit']}) "
          f"commands={c['commands']} runs={c['runs']} "
          f"warmup_runs={c.get('warmup_runs', 0)} stable={c['stable_pass']} "
          f"flaky={c['flaky']} "
          f"nondet={c['nondeterministic']} jittery={c['jittery']} "
          f"stable_fail={c['stable_fail']} unknown={c['unknown']}")
    for cmd in report["commands"]:
        print(f"  {cmd['id']}: {cmd['classification']}  "
              f"passes={cmd['passes']}/{cmd['repeat']} "
              f"warmup={cmd['warmup']} "
              f"outputs={cmd['distinct_outputs']} "
              f"duration_ratio={cmd['duration_ratio']}")
    for failure in report["failures"]:
        print(f"  E {failure['code']}: {failure['message']}")


def _selftest():
    checks = 0

    def ok(cond, msg):
        nonlocal checks
        checks += 1
        if not cond:
            raise AssertionError(msg)

    def write(root, rel, text):
        path = os.path.join(root, rel)
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(text)
        return path

    def run(root, doc):
        path = write(root, "manifest.json",
                     json.dumps(doc, ensure_ascii=False))
        return probe(root, path)

    with tempfile.TemporaryDirectory() as td:
        root = td
        write(root, "stable.py", "print('ok')\n")
        write(root, "fail.py", "import sys\nprint('boom', file=sys.stderr)\n"
                               "sys.exit(2)\n")
        write(root, "flaky.py", "import sys\nfrom pathlib import Path\n"
                               "p=Path('flaky-state.txt')\n"
                               "n=int(p.read_text()) if p.exists() else 0\n"
                               "p.write_text(str(n+1))\n"
                               "sys.exit(0 if n % 2 == 0 else 1)\n")
        write(root, "nondet.py", "import time\nprint(time.time_ns())\n")
        write(root, "jitter.py", "import time\nfrom pathlib import Path\n"
                                 "p=Path('jitter-state.txt')\n"
                                 "n=int(p.read_text()) if p.exists() else 0\n"
                                 "p.write_text(str(n+1))\n"
                                 "time.sleep(0.005 if n % 2 == 0 else 0.8)\n")
        write(root, "unstable_fail.py", "import sys\nfrom pathlib import Path\n"
                                        "p=Path('unstable-fail-state.txt')\n"
                                        "n=int(p.read_text()) if p.exists() else 0\n"
                                        "p.write_text(str(n+1))\n"
                                        "sys.exit(2 if n % 2 == 0 else 3)\n")
        write(root, "warmup_fail.py", "import sys\nfrom pathlib import Path\n"
                                       "p=Path('warmup-fail-state.txt')\n"
                                       "n=int(p.read_text()) if p.exists() else 0\n"
                                       "p.write_text(str(n+1))\n"
                                       "sys.exit(1 if n == 0 else 0)\n")
        write(root, "slow.py", "import time\ntime.sleep(5)\n")
        py = f'"{sys.executable}"'

        def reset(name):
            try:
                os.unlink(os.path.join(root, name))
            except OSError:
                pass

        stable = run(root, {
            "schema": SCHEMA, "repeat": 2, "max_duration_ratio": 100.0,
            "commands": [{"id": "S1", "command": f"{py} stable.py"}],
        })
        ok(stable["status"] == "stable" and stable["exit"] == 0, "stable-pass 状态")
        ok(stable["counts"]["stable_pass"] == 1, "stable-pass 计数")
        ok(stable["commands"][0]["classification"] == "stable-pass", "stable-pass 分类")
        ok(stable["commands"][0]["duration_ratio"] is not None, "耗时比被记录")

        stable_fail = run(root, {
            "schema": SCHEMA, "repeat": 2,
            "commands": [{"id": "F1", "command": f"{py} fail.py"}],
        })
        ok(stable_fail["status"] == "failed" and stable_fail["exit"] == 1,
           "确定性失败不是 flaky")
        ok(stable_fail["commands"][0]["classification"] == "stable-fail",
           "stable-fail 分类")

        unstable_fail = run(root, {
            "schema": SCHEMA, "repeat": 4,
            "commands": [{"id": "U1", "command": f"{py} unstable_fail.py"}],
        })
        ok(unstable_fail["status"] == "unstable", "失败也抖动判 unstable")
        ok(unstable_fail["commands"][0]["classification"] == "unstable-fail",
           "unstable-fail 分类")
        ok(len(set(unstable_fail["commands"][0]["exit_codes"])) == 2,
           "失败码差异被记录")

        flaky = run(root, {
            "schema": SCHEMA, "repeat": 4,
            "commands": [{"id": "L1", "command": f"{py} flaky.py"}],
        })
        ok(flaky["status"] == "unstable" and flaky["exit"] == 1, "flaky 状态")
        ok(flaky["commands"][0]["classification"] == "flaky", "flaky 分类")
        ok(flaky["commands"][0]["passes"] == 2, "flaky 记录通过次数")

        nondet = run(root, {
            "schema": SCHEMA, "repeat": 2,
            "commands": [{"id": "N1", "command": f"{py} nondet.py"}],
        })
        ok(nondet["status"] == "unstable", "输出不一致判 unstable")
        ok(nondet["commands"][0]["classification"] == "nondeterministic",
           "nondeterministic 分类")
        ok(nondet["commands"][0]["distinct_outputs"] == 2, "输出指纹计数")

        lenient = run(root, {
            "schema": SCHEMA, "repeat": 2, "require_identical_output": False,
            "max_duration_ratio": 100.0,
            "commands": [{"id": "N2", "command": f"{py} nondet.py"}],
        })
        ok(lenient["status"] == "stable"
           and lenient["commands"][0]["classification"] == "stable-pass",
           "显式允许输出漂移时仍可 stable-pass")
        ok(lenient["commands"][0]["distinct_outputs"] == 2,
           "允许漂移仍记录不同输出数")

        jitter = run(root, {
            "schema": SCHEMA, "repeat": 2, "max_duration_ratio": 2.0,
            "commands": [{"id": "J1", "command": f"{py} jitter.py"}],
        })
        ok(jitter["status"] == "unstable", f"耗时抖动判 unstable: {jitter['commands'][0]}")
        ok(jitter["commands"][0]["classification"] == "jittery", "jittery 分类")
        ok((jitter["commands"][0]["duration_ratio"] or 0) > 2.0, "抖动比超阈值")

        def tick(sec=0.005, code=0, sha="same"):
            return {"exit": code, "timeout": False, "error": None,
                    "seconds": sec, "output_sha": sha, "output_bytes": 0}

        cold = _classify([], [tick(0.8), tick(0.005)], True, 2.0)
        ok(cold == "jittery", "不预热时首次慢启动算抖动")
        warm = _classify([tick(0.8)], [tick(0.005), tick(0.006)], True, 2.0)
        ok(warm == "stable-pass", "预热后冷启动不计入耗时")
        warm_mixed = _classify([tick(0.005, code=1)],
                               [tick(0.005), tick(0.006)], True, 2.0)
        ok(warm_mixed == "flaky", "预热退出码仍参与 flaky 判定")

        reset("warmup-fail-state.txt")
        warm_fail = run(root, {
            "schema": SCHEMA, "repeat": 2, "warmup": 1,
            "commands": [{"id": "W3", "command": f"{py} warmup_fail.py"}],
        })
        ok(warm_fail["status"] == "unstable", "预热失败不能当稳定")
        ok(warm_fail["commands"][0]["classification"] == "flaky",
           "预热失败 + 后续通过判 flaky")
        ok(warm_fail["commands"][0]["warmup_failures"] == 1,
           "预热失败被单独记录")

        bad = run(root, {"schema": SCHEMA, "warmup": 99,
                         "commands": [{"id": "W4", "command": "true"}]})
        ok(bad["status"] == "unknown", "warmup 越界是 unknown")

        timeout = run(root, {
            "schema": SCHEMA, "repeat": 2, "timeout_s": 0.5,
            "commands": [{"id": "T1", "command": f"{py} slow.py"}],
        })
        ok(timeout["status"] == "unknown" and timeout["exit"] == 3,
           "超时是 unknown 不是 stable")
        ok(timeout["commands"][0]["classification"] == "unknown", "timeout 分类")

        bad = run(root, {"schema": SCHEMA, "commands": []})
        ok(bad["status"] == "unknown" and bad["exit"] == 3, "空 commands 是 unknown")
        bad = run(root, {"schema": SCHEMA, "repeat": 1,
                         "commands": [{"id": "R1", "command": "true"}]})
        ok(bad["status"] == "unknown", "repeat=1 是 unknown")
        bad = run(root, {"schema": SCHEMA, "commands": [
            {"id": "D1", "command": "true"},
            {"id": "D1", "command": "true"}]})
        ok(bad["status"] == "unknown", "重复 id 是 unknown")
        bad = run(root, {"schema": SCHEMA, "commands": [
            {"id": "U1", "command": "true", "typo": 1}]})
        ok(bad["status"] == "unknown", "未知字段是 unknown")
        bad = run(root, {"schema": SCHEMA, "commands": [{"id": "M1"}]})
        ok(bad["status"] == "unknown", "缺 command 是 unknown")

        missing = probe(root, os.path.join(root, "missing.json"))
        ok(missing["status"] == "unknown", "缺清单是 unknown")
        write(root, "bad.json", "{")
        malformed = probe(root, os.path.join(root, "bad.json"))
        ok(malformed["status"] == "unknown", "坏 JSON 是 unknown")
        outside = os.path.join(os.path.dirname(root),
                               f"outside-stability-{time.time_ns()}.json")
        write(root, outside, json.dumps({"schema": SCHEMA,
                                         "commands": [{"id": "O1", "command": "true"}]}))
        try:
            outside_report = probe(root, outside)
        finally:
            try:
                os.unlink(outside)
            except OSError:
                pass
        ok(outside_report["status"] == "unknown", "root 外清单是 unknown")
        ok(any(f["code"] == "manifest-outside-root"
               for f in outside_report["failures"]), "root 外清单点名")

    print(f"stability_probe self-test: {checks}/{checks}")
    return 0


def _parse_args(argv=None):
    p = argparse.ArgumentParser(description="repeat commands and classify flakiness")
    p.add_argument("mode", nargs="?", default="probe", choices=("probe", "self-test"))
    p.add_argument("--root", default=".")
    p.add_argument("--manifest", default=".agent/stability.json")
    p.add_argument("--repeat", type=int, default=None)
    p.add_argument("--warmup", type=int, default=None)
    p.add_argument("--timeout-s", type=float, default=None)
    p.add_argument("--max-duration-ratio", type=float, default=None)
    p.add_argument("--lenient-output", action="store_true")
    p.add_argument("--json", action="store_true")
    p.add_argument("--out", default=None)
    return p.parse_args(argv)


def main(argv=None):
    args = _parse_args(argv)
    if args.mode == "self-test":
        return _selftest()
    if args.repeat is not None and not (REPEAT_LOW <= args.repeat <= REPEAT_HIGH):
        print(f"stability_probe: --repeat 必须是 {REPEAT_LOW}..{REPEAT_HIGH}")
        return 2
    if args.warmup is not None and not (WARMUP_LOW <= args.warmup <= WARMUP_HIGH):
        print(f"stability_probe: --warmup 必须是 {WARMUP_LOW}..{WARMUP_HIGH}")
        return 2
    if args.timeout_s is not None and not (TIMEOUT_LOW <= args.timeout_s <= TIMEOUT_HIGH):
        print(f"stability_probe: --timeout-s 必须是 {TIMEOUT_LOW}..{TIMEOUT_HIGH}")
        return 2
    if args.max_duration_ratio is not None \
            and not (RATIO_LOW <= args.max_duration_ratio <= RATIO_HIGH):
        print(f"stability_probe: --max-duration-ratio 必须是 {RATIO_LOW}..{RATIO_HIGH}")
        return 2
    root = os.path.abspath(args.root)
    manifest = args.manifest
    if not os.path.isabs(manifest):
        manifest = os.path.join(root, manifest)
    report = probe(root, manifest, repeat=args.repeat, timeout_s=args.timeout_s,
                   ratio=args.max_duration_ratio,
                   allow_output_drift=args.lenient_output,
                   warmup=args.warmup)
    _emit(report, args.json, args.out)
    return report["exit"]


if __name__ == "__main__":
    sys.exit(main())
