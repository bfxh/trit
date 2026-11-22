#!/usr/bin/env python3
"""sandbox-matrix: 一批独立沙盒的并发对抗测试（不是单测里的一个夹具）。

    python tools/sandbox_matrix.py --cases 40 --parallel 8 --json

每个 case 是一个**新的 workdir**，跑一种最小充分集：
  safe       相对路径写文件（应放行、宿主不变）
  copy       只复制声明输入后读取（应读到、宿主不变）
  env        打印密钥类环境变量（应被摘掉）
  policy     显式联网命令（应在运行前被 policy 拒绝）
  timeout    睡 2 秒 + 1 秒帽（应 124）
  exit       非零退出（应原样传播）
  flood      输出 5 万字节（应截断且不拖垮驱动）
  hostwrite  用绝对路径写宿主（每个 case 独立宿主，应判 host-mutated）

通过判据（机器可判，不靠"看起来没问题"）：
  * failed == 0；
  * 每个 case 的 workdir 互不相同（并发不串）；
  * safe/copy/env 的 host.mutated 必须为 False；
  * timeout 必须 exit 124；policy 必须 policy-denied；exit 必须原值；
  * flood 必须 stdout_truncated；
  * hostwrite 必须有独立宿主、被检测到 mutated 且 exit 3（不是"共享宿主碰巧变了"）。

诚实边界：矩阵只证明这些**协作式**不变量；不是内核隔离证明。宿主写入
检测见 tools/sandbox_run.py 的 before/after 指纹，改回原样的情况不在承诺内。
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sandbox_run as sb

PY = sys.executable
SCENARIOS = ("safe", "copy", "env", "policy", "timeout", "exit", "flood", "hostwrite")


def make_host(tmp):
    host = os.path.join(tmp, "host")
    os.makedirs(host)
    with open(os.path.join(host, "in.txt"), "w", encoding="utf-8", newline="\n") as fh:
        fh.write("DATA\n")
    subprocess.run(["git", "-C", host, "init", "-q"], check=False)
    subprocess.run(["git", "-C", host, "config", "user.email", "a@b"], check=False)
    subprocess.run(["git", "-C", host, "config", "user.name", "a"], check=False)
    subprocess.run(["git", "-C", host, "add", "."], check=False)
    subprocess.run(["git", "-C", host, "commit", "-qm", "init"], check=False)
    return host


def one_case(host, workroot, idx, scenario, tmp):
    work = os.path.join(workroot, f"w-{idx:03d}-{scenario}")
    hroot = ""
    if scenario == "safe":
        rec = sb.run_sandbox(host, work, [PY, "-c", "open('out.txt','w').write('x')"])
        ok = rec["ok"] and not rec["host"]["mutated"]
    elif scenario == "copy":
        rec = sb.run_sandbox(host, work, [PY, "-c", "print(open('in.txt').read().strip())"],
                             copies=("in.txt",))
        ok = rec["ok"] and "DATA" in rec["run"]["stdout_tail"] and not rec["host"]["mutated"]
    elif scenario == "env":
        rec = sb.run_sandbox(host, work, [PY, "-c",
                                          "import os; print('S=' + repr(os.getenv('SANDBOX_MATRIX_SECRET')))"])
        ok = rec["ok"] and "matrix-secret" not in rec["run"]["stdout_tail"]
    elif scenario == "policy":
        rec = sb.run_sandbox(host, work, ["curl", "https://example.com"])
        ok = rec["verdict"] == "policy-denied"
    elif scenario == "timeout":
        rec = sb.run_sandbox(host, work, [PY, "-c", "import time; time.sleep(2)"], seconds=1)
        ok = rec["exit"] == 124 and rec["run"]["timed_out"]
    elif scenario == "exit":
        rec = sb.run_sandbox(host, work, [PY, "-c", "raise SystemExit(7)"])
        ok = rec["exit"] == 7
    elif scenario == "flood":
        rec = sb.run_sandbox(host, work, [PY, "-c", "print('x'*50000)"], max_output=1000)
        ok = rec["run"]["stdout_truncated"] and rec["run"]["stdout_bytes"] > 1000
    elif scenario == "hostwrite":
        # One host per case: a shared host would make "mutated by someone else"
        # indistinguishable from "this case escaped", which is the failure mode the
        # matrix exists to catch. No .git means the non-git manifest path is exercised.
        hroot = os.path.join(tmp, "hostwrite", f"h-{idx:03d}")
        rec = {"ok": False, "verdict": "hostwrite setup failed", "exit": 2, "work": work,
               "host": {"mutated": None}, "run": {}}
        try:
            os.makedirs(hroot, exist_ok=True)
            shutil.copy2(os.path.join(host, "in.txt"), os.path.join(hroot, "in.txt"))
            evil = os.path.join(hroot, f"evil-{idx:03d}.txt")
            rec = sb.run_sandbox(hroot, work,
                                 [PY, "-c", f"open({evil!r},'w').write('bad')"])
            ok = (rec["verdict"] == "host-mutated" and rec["exit"] == 3
                  and rec["host"]["mutated"] is True and os.path.exists(evil))
        finally:
            # run_sandbox intentionally keeps the workdir on host-mutated (evidence);
            # this matrix owns the fixture, so it removes both.
            shutil.rmtree(hroot, ignore_errors=True)
            shutil.rmtree(work, ignore_errors=True)
    else:
        rec = {"ok": False, "verdict": "unknown scenario", "exit": 2, "work": work}
        ok = False
    return {"idx": idx, "scenario": scenario, "ok": bool(ok), "exit": rec["exit"],
            "verdict": rec["verdict"], "work": rec["work"],
            "host_mutated": (rec.get("host") or {}).get("mutated"),
            "stdout_truncated": (rec.get("run") or {}).get("stdout_truncated")}


def main(argv):
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--cases", type=int, default=40)
    p.add_argument("--parallel", type=int, default=8)
    p.add_argument("--scenario", default="", choices=[""] + list(SCENARIOS))
    p.add_argument("--json", action="store_true")
    p.add_argument("--receipt", default="")
    args = p.parse_args(argv)
    if args.cases < 1 or args.cases > 1000:
        print("sandbox-matrix: --cases must be 1..1000", file=sys.stderr)
        return 2
    if args.parallel < 1 or args.parallel > 64:
        print("sandbox-matrix: --parallel must be 1..64", file=sys.stderr)
        return 2
    os.environ["SANDBOX_MATRIX_SECRET"] = "matrix-secret"
    tmp = tempfile.mkdtemp(prefix="sandbox-matrix-")
    try:
        host = make_host(tmp)
        workroot = os.path.join(tmp, "work")
        os.makedirs(workroot)
        jobs = [(i, SCENARIOS[i % len(SCENARIOS)]) for i in range(args.cases)]
        if args.scenario:
            jobs = [(i, args.scenario) for i in range(args.cases)]
        results = []
        with ThreadPoolExecutor(max_workers=args.parallel) as pool:
            futures = [pool.submit(one_case, host, workroot, i, sc, tmp) for i, sc in jobs]
            for fut in as_completed(futures):
                results.append(fut.result())
        results.sort(key=lambda r: r["idx"])
        works = {r["work"] for r in results}
        failures = [r for r in results if not r["ok"]]
        clean_host = [r for r in results if r["scenario"] in ("safe", "copy", "env")
                      and r["host_mutated"] is not False]
        hostwrites = [r for r in results if r["scenario"] == "hostwrite"]
        hostwrite_misses = [r for r in hostwrites if not r["ok"]]
        summary = {"tool": "sandbox-matrix", "cases": args.cases, "parallel": args.parallel,
                   "scenario": args.scenario or "all",
                   "passed": args.cases - len(failures), "failed": len(failures),
                   "distinct_workdirs": len(works), "host_violations": len(clean_host),
                   "hostwrite_cases": len(hostwrites),
                   "hostwrite_detected": len(hostwrites) - len(hostwrite_misses),
                   "hostwrite_misses": len(hostwrite_misses),
                   "failures": failures}
        if args.receipt:
            sb.write_receipt(args.receipt, summary)
        if args.json:
            print(json.dumps(summary, ensure_ascii=True))
        else:
            print(f"sandbox-matrix: {summary['passed']}/{args.cases} passed, "
                  f"distinct workdirs={len(works)}, failures={len(failures)}, "
                  f"hostwrite={summary['hostwrite_detected']}/{summary['hostwrite_cases']}")
            for row in failures[:10]:
                print(f"  - #{row['idx']} {row['scenario']}: {row['verdict']} exit={row['exit']}")
        bad = bool(failures) or len(works) != args.cases or clean_host
        return 0 if not bad else 1
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
        os.environ.pop("SANDBOX_MATRIX_SECRET", None)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
