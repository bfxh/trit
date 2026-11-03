#!/usr/bin/env python3
"""A4 扫描器：N 配置 → 自动复核 → 按指标晋级 → 自动重建目录（带排队/重试/断点续跑）。

用法:
    python scripts/depth_sweep.py --spec sweeps/depth-basic.json [--jobs 2] [--retries 1]
    python scripts/depth_sweep.py --spec sweeps/depth-basic.json --fresh   # 忽略已有产物
    python scripts/depth_sweep.py --self-test

规格（JSON）:
{
  "family": "depth",
  "data": ["E:/.../scene0000", ...],              // 与 data_globs 二选一
  "data_globs": ["E:/.../scene*", "E:/.../pt*"],  // 排序后展开
  "holdout": ["scene0050", "scene0051"],
  "out_dir": "runs/depth/sweep-basic",
  "base": {"size": 64, "dil": 2, "accum": 8, "lr": 0.002, "epochs": 3, "stride": 2},
  "grid": {"hidden": [32, 64, 128], "lr": [0.001, 0.002]},
  "jobs": 2,
  "verify": true,
  "promote": {"max": 8.0, "beat_baseline_pp": 0.5},
  "catalogue": "docs/DEPTH-RESULTS.md"
}

模板模式（部件/纹理/提取族，`family` ≠ depth）：不给 `data/holdout` 也行，但必须给
`producer` / `verifier` / `model`：
{
  "family": "extract",
  "out_dir": "runs/extract/sweep-steps",
  "base": {"cell": 0.01}, "grid": {"steps": [5000, 20000]},
  "producer": {"bin": "parts-3d", "argv": ["occ", "--steps", "{steps}", "--save-model", "{model}",
                                          "--run-manifest", "{model}.manifest.json", ...]},
  "verifier": {"bin": "parts-extract-verify", "argv": ["--model", "{model}"]},
  "model": "{out}/{tag}.occ1",
  "promote": {"metric": "iou_mean", "min": 0.5, "all_closed": true}
}
占位符：`{tag}` `{out}` `{model}` `{data}` `{holdout}` `{gpu}` + 每个配置键；
没解析掉的占位符**直接判红**。模板模式的生产器自己选卡 ⇒ 不抢 GPU 租约（`jobs` 别开太大）。

行为：
- 对 `grid` 做笛卡尔积（`base` 里同名键被覆盖），**并发**跑（每配置一个进程：训练 → 复核）。
- **断点续跑**：产物（清单 + 复核记录，且清单里 config 与本次一致）齐了就跳过，记 `cached`；
  进度写 `<out_dir>/sweep.queue.json`（带规格指纹，规格变了会拒绝续跑）。
- **重试**：训练/复核失败按 `--retries`（缺省 1）重试；「复核判定拒绝」（指纹不一致）不重试。
- 最后按 `promote` 评晋级，写 `sweep.json` / `promoted.json`，并重建整棵结果树目录。
"""

import argparse
import concurrent.futures
import glob
import hashlib
import itertools
import json
import os
import pathlib
import queue
import subprocess
import sys
import tempfile
import threading
import time

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")

PRINT_LOCK = threading.Lock()
QUEUE_LOCK = threading.Lock()

# ---------------------------------------------------------------- 机器级 GPU 租约
#
# 多个扫描器进程共用同一台机器时，按「一台卡一份租约」串行化：拿不到就排队等，
# 租约带 `expires`（心跳续租），进程崩了最多 `ttl` 秒后别人可抢占。

LEASE_DIR = pathlib.Path(tempfile.gettempdir()) / "trit-gpu-leases"


def lease_path(gpu):
    return LEASE_DIR / f"gpu{gpu}.json"


def lease_read(gpu):
    try:
        return json.loads(lease_path(gpu).read_text(encoding="utf-8"))
    except Exception:
        return None


def lease_held(gpu, now=None):
    """该卡当前是否被**未过期**的租约占着。"""
    now = time.time() if now is None else now
    d = lease_read(gpu)
    return bool(d) and float(d.get("expires", 0)) > now


def lease_write(gpu, data):
    LEASE_DIR.mkdir(parents=True, exist_ok=True)
    p = lease_path(gpu)
    tmp = p.with_name(f"{p.name}.{os.getpid()}.tmp")
    tmp.write_text(json.dumps(data), encoding="utf-8")
    tmp.replace(p)


def lease_try(gpu, owner, ttl=120.0):
    """尝试拿租约：不存在就原子创建；已过期就抢占（写临时再 replace，回读确认）。"""
    LEASE_DIR.mkdir(parents=True, exist_ok=True)
    now = time.time()
    data = {"gpu": gpu, "owner": owner, "pid": os.getpid(), "expires": now + ttl}
    try:
        fd = os.open(lease_path(gpu), os.O_CREAT | os.O_EXCL | os.O_WRONLY)
    except FileExistsError:
        if lease_held(gpu, now):
            return False
        lease_write(gpu, data)
    else:
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(data, f)
        return True
    back = lease_read(gpu) or {}
    return back.get("owner") == owner


def lease_acquire(pool, owner, wait=0.0, ttl=120.0, poll=0.5):
    """在 `pool` 里等一张空闲卡；`wait` 秒内拿不到返回 None（排队）。"""
    deadline = time.time() + max(0.0, wait)
    while True:
        for gpu in pool:
            if lease_try(gpu, owner, ttl):
                return gpu
        if time.time() >= deadline:
            return None
        time.sleep(poll)


def lease_release(gpu, owner):
    d = lease_read(gpu)
    if d and d.get("owner") == owner:
        try:
            lease_path(gpu).unlink()
        except FileNotFoundError:
            pass
        return True
    return False


def lease_heartbeat(gpu, owner, ttl, stop):
    """长任务续租：每 `ttl/3` 把 expires 往后推；租约被别人抢走就退出。"""
    while not stop.wait(max(0.2, ttl / 3.0)):
        d = lease_read(gpu)
        if not d or d.get("owner") != owner:
            return
        d["expires"] = time.time() + ttl
        lease_write(gpu, d)


def lease_start(gpu, owner, ttl):
    stop = threading.Event()
    t = threading.Thread(target=lease_heartbeat, args=(gpu, owner, ttl, stop), daemon=True)
    t.start()
    return stop


def lease_stop(gpu, owner, stop):
    stop.set()
    lease_release(gpu, owner)


def out(msg):
    with PRINT_LOCK:
        print(msg, flush=True)


def build_configs(base, grid):
    keys = sorted(grid)
    cfgs = []
    for combo in itertools.product(*(grid[k] for k in keys)):
        cfg = dict(base)
        cfg.update(dict(zip(keys, combo)))
        cfgs.append(cfg)
    return cfgs


def tag_of(cfg, grid_keys, base):
    parts = []
    for k in grid_keys:
        v = cfg[k]
        parts.append(f"{k}{v:g}" if isinstance(v, float) else f"{k}{v}")
    for k in ("hidden", "dil"):
        if k in base and k not in grid_keys:
            parts.append(f"{k}{cfg[k]}")
    return "-".join(parts) or "run"


def resolve_data(spec):
    if "data" in spec:
        return list(spec["data"])
    dirs = []
    for g in spec.get("data_globs", []):
        dirs.extend(sorted(glob.glob(g)))
    if not dirs:
        sys.exit("规格里 data / data_globs 都没有展开出目录")
    return dirs


def cli_map(cfg):
    """把配置字典转成命令行参数（与 parts-depth-gpu 的开关一一对应）。"""
    flags = []
    for k in ("size", "hidden", "dil", "epochs", "stride", "pyr", "accum", "seed"):
        if k in cfg:
            flags += [f"--{k}", str(cfg[k])]
    if "lr" in cfg:
        flags += ["--lr", f"{cfg['lr']:g}"]
    for k in ("full", "f32"):
        if cfg.get(k):
            flags.append(f"--{k}")
    return flags


def slot_gpus(jobs, pool):
    """给 `jobs` 个并发槽分配 GPU 下标（从 `pool` 里轮转、确定性）。"""
    pool = list(pool) or [0]
    return [pool[i % len(pool)] for i in range(max(1, jobs))]


def parse_gpu_pool(text):
    """从 `--list-gpus` 输出里挑可用卡：优先独显；没有独显就用非 CPU 的全部。"""
    lines = [ln for ln in text.splitlines() if ln.strip()]
    idx = [i for i, ln in enumerate(lines) if "(DiscreteGpu)" in ln]
    if not idx:
        idx = [i for i, ln in enumerate(lines) if "(Cpu)" not in ln]
    return idx or [0]


def detect_gpu_pool(bin_path):
    """探测卡池；失败按 `[0]`。"""
    try:
        p = subprocess.run(
            [str(bin_path), "--list-gpus"], capture_output=True, text=True, timeout=60
        )
        if p.returncode != 0:
            return [0]
        return parse_gpu_pool(p.stdout)
    except Exception:
        return [0]


def run(cmd, log, timeout, on_start=None):
    """跑一条命令；返回 `(rc, secs)`。`on_start(pid)` 在进程起来后回调（记进队列）。"""
    t0 = time.time()
    with open(log, "w", encoding="utf-8") as f:
        p = subprocess.Popen(cmd, stdout=f, stderr=subprocess.STDOUT)
        if on_start:
            on_start(p.pid)
        try:
            rc = p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            p.kill()
            p.wait()
            rc = -9
    return rc, time.time() - t0


def kill_pid(pid):
    """杀一个可能还活着的子进程（续跑前清理上次崩溃留下的孤儿训练进程）。"""
    if not pid:
        return False
    try:
        if sys.platform.startswith("win"):
            subprocess.run(["taskkill", "/F", "/PID", str(pid)], capture_output=True)
        else:
            import os
            os.kill(pid, 9)
        return True
    except Exception:
        return False


def stale_pids(states):
    """上次运行留下、仍标记 running 的子进程 pid。"""
    return [
        st["pid"]
        for st in states.values()
        if isinstance(st, dict) and st.get("state") == "running" and st.get("pid")
    ]


def run_with_retry(cmd, log, timeout, retries, on_start=None):
    """跑一条命令；非零退出按 `retries` 重试（第 n 次日志加 `.retryN`）。"""
    attempts = 0
    while True:
        attempts += 1
        lg = log if attempts == 1 else log.with_name(f"{log.name}.retry{attempts}")
        rc, secs = run(cmd, lg, timeout, on_start)
        if rc == 0 or attempts > retries:
            return rc, secs, attempts


def spec_sha(spec):
    """规格指纹：键排序后哈希（字段顺序不影响）。"""
    return hashlib.sha256(
        json.dumps(spec, sort_keys=True, ensure_ascii=False).encode("utf-8")
    ).hexdigest()


def cfg_matches(manifest_cfg, cfg):
    """清单里的 config 是否与本次配置一致（缺键也算不一致）。"""
    if not isinstance(manifest_cfg, dict):
        return False
    return all(k in manifest_cfg and manifest_cfg[k] == v for k, v in cfg.items())


def is_complete(model, cfg, verify_enabled):
    """产物已完成：清单在、config 一致；需要复核时 `.verify.json` 也在。"""
    mp = pathlib.Path(str(model) + ".manifest.json")
    if not mp.exists():
        return False
    try:
        man = json.loads(mp.read_text(encoding="utf-8"))
    except Exception:
        return False
    if not cfg_matches(man.get("config", {}), cfg):
        return False
    if verify_enabled and not pathlib.Path(str(model) + ".verify.json").exists():
        return False
    return True


def should_retry(rc, rejected):
    """非零退出、且不是「复核判定拒绝」才值得重试。"""
    return rc != 0 and not rejected


def load_queue(path, sha):
    if not path.exists():
        return {}
    try:
        q = json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return {}
    if q.get("spec_sha") != sha:
        sys.exit(f"{path} 的规格指纹与当前 --spec 不一致；换 out_dir 或加 --fresh")
    return q.get("jobs", {})


def save_queue(path, sha, states):
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(
        json.dumps({"spec_sha": sha, "jobs": states}, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    tmp.replace(path)


def cached_row(tag, cfg, model, verify, prev):
    row = {
        "tag": tag,
        "config": cfg,
        "model": str(model),
        "gpu": (prev or {}).get("gpu"),
        "status": "verified" if verify else "trained",
        "cached": True,
    }
    vp = pathlib.Path(str(model) + ".verify.json")
    if verify and vp.exists():
        try:
            row["metrics"] = json.loads(vp.read_text(encoding="utf-8")).get("metrics")
        except Exception:
            row["metrics"] = None
    mp = pathlib.Path(str(model) + ".manifest.json")
    if mp.exists():
        try:
            row["train_mse"] = json.loads(mp.read_text(encoding="utf-8")).get("result", {}).get(
                "final_mse"
            )
        except Exception:
            pass
    return row


def evaluate_promotion(row, rule):
    """按指标判晋级。返回 `(promoted, reason)`。

    规则键：`max` / `min`（`metric` 的上/下限）、`beat_baseline_pp`（要比常数基线至少低多少
    个百分点）、`all_closed`（`closed == scenes`，提取族的闭合门）、`metric`（默认 `rel_pct`）。
    只对**已复核**的行生效。
    """
    if not rule:
        return False, "未配置规则"
    if row.get("status") != "verified":
        return False, "未通过复核"
    m = row.get("metrics") or {}
    metric = rule.get("metric", "rel_pct")
    rel = m.get(metric)
    if rel is None:
        return False, "无指标"
    if "max" in rule and rel > rule["max"]:
        return False, f"{rel:g} > 上限 {rule['max']:g}"
    if "min" in rule and rel < rule["min"]:
        return False, f"{rel:g} < 下限 {rule['min']:g}"
    if rule.get("all_closed"):
        if m.get("closed") is None or m.get("scenes") is None:
            return False, "缺闭合读数"
        if m.get("closed") != m.get("scenes"):
            return False, f"闭合 {m['closed']}/{m['scenes']}"
    if "beat_baseline_pp" in rule:
        base = m.get("base_pct")
        if base is None:
            return False, "缺常数基线"
        gap = base - rel
        if gap < rule["beat_baseline_pp"]:
            return False, f"只比基线低 {gap:.2f}pp（需 ≥{rule['beat_baseline_pp']}pp）"
    return True, "达标"


def render_template(tpl, ctx):
    """把 `{key}` 换成 ctx 里的值。没替换掉的占位符**直接判红**（打错键名不许静默跑）。"""
    s = tpl
    for k, v in ctx.items():
        s = s.replace("{" + k + "}", str(v))
    if "{" in s:
        i = s.find("{")
        raise SystemExit(f"模板占位符未解析：{s[i:i + 40]!r}（可用键：{sorted(ctx)}）")
    return s


def resolve_bin(name, bin_dir):
    """`bin` 可以是完整路径，也可以是 bin-dir 下的名字（Windows 自动补 .exe）。"""
    p = pathlib.Path(name)
    if p.exists():
        return p
    q = bin_dir / name
    if q.exists():
        return q
    if sys.platform.startswith("win") and not name.endswith(".exe"):
        q = bin_dir / f"{name}.exe"
        if q.exists():
            return q
    return q


def run_one(cfg, tag, gpu, ctx):
    model = ctx["out_dir"] / f"{tag}.dcn"
    tlog = ctx["out_dir"] / f"{tag}.train.log"
    vlog = ctx["out_dir"] / f"{tag}.verify.log"
    train_cmd = [
        str(ctx["train_bin"]), "cnn-train", "--data", ctx["data_arg"],
        "--holdout", ctx["hold"], "--out", str(model), "--gpu-index", str(gpu),
        *cli_map(cfg),
    ]
    out(f"[{tag}] 训练…（gpu {gpu}）")
    rc, secs, attempts = run_with_retry(
        train_cmd, tlog, ctx["timeout"], ctx["retries"], ctx.get("on_start")
    )
    row = {
        "tag": tag, "config": cfg, "model": str(model), "gpu": gpu,
        "train_rc": rc, "train_secs": round(secs, 1), "train_attempts": attempts,
    }
    if rc != 0:
        row["status"] = "train-failed"
        out(f"[{tag}] 训练失败 rc={rc}（{attempts} 次；日志 {tlog}）")
        return row
    if ctx["verify"]:
        verify_cmd = [
            str(ctx["verify_bin"]), "--dcn", str(model), "--data", ctx["data_arg"],
            "--holdout", ctx["hold"], "--size", str(cfg.get("size", 64)),
            "--stride", str(cfg.get("stride", 2)), "--pyr", str(cfg.get("pyr", 0)),
        ]
        if cfg.get("full"):
            verify_cmd.append("--full")
        out(f"[{tag}] 复核…")
        vattempts = 0
        while True:
            vattempts += 1
            lg = vlog if vattempts == 1 else vlog.with_name(f"{vlog.name}.retry{vattempts}")
            vrc, vsecs = run(verify_cmd, lg, ctx["timeout"], ctx.get("on_start"))
            rejected = pathlib.Path(str(model) + ".verify.reject.json").exists()
            if vrc == 0 or not should_retry(vrc, rejected) or vattempts > ctx["retries"]:
                break
            out(f"[{tag}] 复核失败 rc={vrc}，重试 {vattempts}/{ctx['retries']}")
        row["verify_rc"] = vrc
        row["verify_secs"] = round(vsecs, 1)
        row["verify_attempts"] = vattempts
        vpath = pathlib.Path(str(model) + ".verify.json")
        if vrc == 0 and vpath.exists():
            v = json.loads(vpath.read_text(encoding="utf-8"))
            row["metrics"] = v.get("metrics")
            row["status"] = "verified"
            m = row["metrics"]
            out(f"[{tag}] PASS {m['rel_pct']:.2f}% / δ {m['delta']:.3f} / 基线 {m['base_pct']:.2f}%")
        else:
            row["status"] = "verify-failed"
            out(f"[{tag}] FAIL 复核 rc={vrc}（{vattempts} 次；日志 {vlog}）")
    else:
        row["status"] = "trained"
    return row


def run_one_generic(cfg, tag, gpu, ctx):
    """模板模式：生产器 → 复核器 → 读 `<model>.verify.json` 的 metrics。"""
    t = dict(cfg)
    t.update(
        {
            "tag": tag,
            "out": str(ctx["out_dir"]),
            "data": ctx["data"],
            "holdout": ctx["holdout"],
            "gpu": gpu,
        }
    )
    t["model"] = render_template(ctx["model_tpl"], t)
    model = pathlib.Path(t["model"])
    tlog = ctx["out_dir"] / f"{tag}.train.log"
    vlog = ctx["out_dir"] / f"{tag}.verify.log"
    prod = [str(ctx["producer_bin"])] + [
        render_template(a, t) for a in ctx["producer_argv"]
    ]
    out(f"[{tag}] 生产…")
    rc, secs, attempts = run_with_retry(
        prod, tlog, ctx["timeout"], ctx["retries"], ctx.get("on_start")
    )
    row = {
        "tag": tag,
        "config": cfg,
        "model": str(model),
        "gpu": gpu,
        "train_rc": rc,
        "train_secs": round(secs, 1),
        "train_attempts": attempts,
    }
    if rc != 0:
        row["status"] = "train-failed"
        out(f"[{tag}] 生产失败 rc={rc}（{attempts} 次；日志 {tlog}）")
        return row
    if not ctx["verify"]:
        row["status"] = "trained"
        return row
    ver = [str(ctx["verify_bin"])] + [
        render_template(a, t) for a in ctx["verify_argv"]
    ]
    record = pathlib.Path(render_template(ctx["verify_record_tpl"], t))
    reject = pathlib.Path(str(record).removesuffix(".verify.json") + ".verify.reject.json")
    out(f"[{tag}] 复核…")
    vattempts = 0
    while True:
        vattempts += 1
        lg = vlog if vattempts == 1 else vlog.with_name(f"{vlog.name}.retry{vattempts}")
        vrc, vsecs = run(ver, lg, ctx["timeout"], ctx.get("on_start"))
        if vrc == 0 or not should_retry(vrc, reject.exists()) or vattempts > ctx["retries"]:
            break
        out(f"[{tag}] 复核失败 rc={vrc}，重试 {vattempts}/{ctx['retries']}")
    row["verify_rc"] = vrc
    row["verify_secs"] = round(vsecs, 1)
    row["verify_attempts"] = vattempts
    if vrc == 0 and record.exists():
        v = json.loads(record.read_text(encoding="utf-8"))
        row["metrics"] = v.get("metrics")
        row["status"] = "verified"
        m = row["metrics"] or {}
        brief = ", ".join(
            f"{k}={v:.4g}" if isinstance(v, float) else f"{k}={v}"
            for k, v in m.items()
            if isinstance(v, (int, float))
        )
        out(f"[{tag}] PASS {brief}")
    else:
        row["status"] = "verify-failed"
        out(f"[{tag}] FAIL 复核 rc={vrc}（{vattempts} 次；日志 {vlog}）")
    return row


def self_test():
    import tempfile

    good = {"status": "verified", "metrics": {"rel_pct": 7.3, "base_pct": 7.91}}
    over = {"status": "verified", "metrics": {"rel_pct": 8.5, "base_pct": 7.91}}
    weak = {"status": "verified", "metrics": {"rel_pct": 7.8, "base_pct": 7.91}}
    unver = {"status": "verify-failed", "metrics": {"rel_pct": 7.0, "base_pct": 7.91}}
    rule = {"max": 8.0, "beat_baseline_pp": 0.5}

    # 断点续跑：清单 + 配置一致性 + 复核记录
    with tempfile.TemporaryDirectory() as td:
        model = pathlib.Path(td) / "m.dcn"
        cfg = {"hidden": 32, "lr": 0.002, "epochs": 3}
        assert not is_complete(model, cfg, True), "无产物应判未完成"
        (pathlib.Path(str(model) + ".manifest.json")).write_text(
            json.dumps({"config": {"hidden": 32, "lr": 0.002, "epochs": 2}}), encoding="utf-8"
        )
        assert not is_complete(model, cfg, True), "config 不一致应判未完成"
        (pathlib.Path(str(model) + ".manifest.json")).write_text(
            json.dumps({"config": cfg}), encoding="utf-8"
        )
        assert not is_complete(model, cfg, True), "缺 verify.json 应判未完成"
        assert is_complete(model, cfg, False), "不要求复核时应判完成"
        (pathlib.Path(str(model) + ".verify.json")).write_text("{}", encoding="utf-8")
        assert is_complete(model, cfg, True), "产物齐了应判完成"

    # GPU 租约：互斥 / 过期抢占 / release 只对 owner 生效 / 排队
    global LEASE_DIR
    old_dir = LEASE_DIR
    with tempfile.TemporaryDirectory() as td:
        LEASE_DIR = pathlib.Path(td)
        try:
            assert lease_try(0, "a"), "空卡应能拿到"
            assert not lease_try(0, "b"), "被占时不能再拿"
            assert lease_held(0)
            lease_write(1, {"gpu": 1, "owner": "x", "expires": time.time() - 1})
            assert not lease_held(1), "过期租约不算占用"
            assert lease_try(1, "b"), "过期租约应可抢占"
            assert (lease_read(1) or {}).get("owner") == "b"
            assert not lease_release(1, "a"), "release 只对 owner 生效"
            assert lease_release(1, "b")
            assert not lease_held(1)
            assert lease_release(0, "a"), "先释放第一张卡再重拿"
            assert lease_try(0, "a")
            got = {}

            def waiter():
                got["g"] = lease_acquire([0], "b", wait=5, ttl=60, poll=0.05)

            th = threading.Thread(target=waiter)
            th.start()
            time.sleep(0.3)
            assert "g" not in got, "被占时不应立刻拿到"
            lease_release(0, "a")
            th.join(timeout=5)
            assert got.get("g") == 0, "释放后排队者应拿到"
            lease_release(0, "b")
        finally:
            LEASE_DIR = old_dir

    cases = [
        ("达标", evaluate_promotion(good, rule), (True, "达标")),
        ("超上限", evaluate_promotion(over, rule)[0], False),
        ("赢基线不够", evaluate_promotion(weak, rule)[0], False),
        ("未复核", evaluate_promotion(unver, rule)[0], False),
        ("无规则", evaluate_promotion(good, None)[0], False),
        ("缺指标", evaluate_promotion({"status": "verified"}, rule)[0], False),
        ("只给上限", evaluate_promotion(weak, {"max": 8.0})[0], True),
        ("只给基线", evaluate_promotion(good, {"beat_baseline_pp": 0.5})[0], True),
        ("槽分配 4x2", slot_gpus(4, [0, 1]), [0, 1, 0, 1]),
        ("槽分配 3x1", slot_gpus(3, [0]), [0, 0, 0]),
        ("槽分配 2x[2,5]", slot_gpus(2, [2, 5]), [2, 5]),
        (
            "卡池：有独显只取独显",
            parse_gpu_pool("0: A (DiscreteGpu)\n1: B (IntegratedGpu)\n2: W (Cpu)\n"),
            [0],
        ),
        (
            "卡池：没独显用非 CPU",
            parse_gpu_pool("0: A (IntegratedGpu)\n1: W (Cpu)\n"),
            [0],
        ),
        (
            "自定义指标",
            evaluate_promotion(
                {"status": "verified", "metrics": {"mae_log": 0.07}},
                {"metric": "mae_log", "max": 0.08},
            )[0],
            True,
        ),
        ("模板替换", render_template("{out}/{tag}.tpm", {"out": "E:/r", "tag": "x"}), "E:/r/x.tpm"),
        (
            "下限达标",
            evaluate_promotion(
                {"status": "verified", "metrics": {"miou": 0.42}},
                {"metric": "miou", "min": 0.4},
            )[0],
            True,
        ),
        (
            "下限不够",
            evaluate_promotion(
                {"status": "verified", "metrics": {"miou": 0.39}},
                {"metric": "miou", "min": 0.4},
            )[0],
            False,
        ),
        (
            "闭合全过",
            evaluate_promotion(
                {"status": "verified", "metrics": {"iou_mean": 0.55, "closed": 8, "scenes": 8}},
                {"metric": "iou_mean", "min": 0.5, "all_closed": True},
            )[0],
            True,
        ),
        (
            "闭合不全",
            evaluate_promotion(
                {"status": "verified", "metrics": {"iou_mean": 0.55, "closed": 7, "scenes": 8}},
                {"metric": "iou_mean", "min": 0.5, "all_closed": True},
            )[0],
            False,
        ),
        (
            "闭合缺读数",
            evaluate_promotion(
                {"status": "verified", "metrics": {"iou_mean": 0.55}},
                {"metric": "iou_mean", "min": 0.5, "all_closed": True},
            )[0],
            False,
        ),
        ("重试：失败可重试", should_retry(101, False), True),
        ("重试：拒绝不重试", should_retry(1, True), False),
        ("重试：成功不重试", should_retry(0, False), False),
        ("配置匹配：一致", cfg_matches({"a": 1, "b": 2}, {"a": 1}), True),
        ("配置匹配：不一致", cfg_matches({"a": 1}, {"a": 2}), False),
        ("配置匹配：缺键", cfg_matches({}, {"a": 1}), False),
        ("配置匹配：非字典", cfg_matches(None, {"a": 1}), False),
        (
            "规格指纹：键序无关",
            spec_sha({"a": 1, "b": [2]}) == spec_sha({"b": [2], "a": 1}),
            True,
        ),
        ("规格指纹：内容敏感", spec_sha({"a": 1}) != spec_sha({"a": 2}), True),
        (
            "残留进程：只挑 running 且有 pid",
            stale_pids(
                {
                    "a": {"state": "running", "pid": 111},
                    "b": {"state": "done", "pid": 222},
                    "c": {"state": "running"},
                    "d": {"state": "failed", "pid": 333},
                }
            ),
            [111],
        ),
    ]
    # 未知占位符必须判红（打错键名不许静默跑）
    try:
        render_template("{nope}", {})
        unknown_red = False
    except SystemExit:
        unknown_red = True
    cases.append(("模板未知键判红", unknown_red, True))

    bad = [n for n, got, want in cases if got != want]
    for n, got, want in cases:
        print(f"  {n}: {'ok' if got == want else 'FAIL'}（got={got} want={want}）")
    if bad:
        print(f"FAIL {len(bad)} 条：{bad}")
        sys.exit(1)
    print(f"self-test {len(cases) + 5}/{len(cases) + 5} 通过（含文件系统 + 租约断言）")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--spec")
    ap.add_argument("--bin-dir", default="target/release")
    ap.add_argument("--jobs", type=int, default=0, help="并发进程数（0 = 用规格里的 jobs，缺省 max(2, 卡数)）")
    ap.add_argument("--gpus", type=int, default=0, help="卡数（0 = 自动探测 --list-gpus）")
    ap.add_argument("--retries", type=int, default=1, help="训练/复核失败的重试次数（缺省 1）")
    ap.add_argument("--no-lease", action="store_true", help="不跨扫描器抢卡（默认会排队等租约）")
    ap.add_argument("--lease-wait", type=int, default=3600, help="等一张空闲卡的秒数（缺省 3600）")
    ap.add_argument("--lease-ttl", type=float, default=120.0, help="租约过期秒数（心跳每 ttl/3 续租）")
    ap.add_argument("--fresh", action="store_true", help="忽略已有产物与队列，全部重跑")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--timeout", type=int, default=3600, help="单个训练/复核的超时（秒）")
    ap.add_argument("--self-test", action="store_true")
    args = ap.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.spec:
        ap.error("需要 --spec（或 --self-test）")

    spec = json.loads(pathlib.Path(args.spec).read_text(encoding="utf-8"))
    # 模板模式（部件/纹理/提取）：producer + verifier 都由规格给 argv，深度专用逻辑不参与。
    generic = isinstance(spec.get("producer"), dict)
    if generic:
        data = list(spec.get("data", []))
        for g in spec.get("data_globs", []):
            data.extend(sorted(glob.glob(g)))
        holdout = list(spec.get("holdout", []))
    else:
        data = resolve_data(spec)
        holdout = spec["holdout"]
    base = spec.get("base", {})
    grid = spec.get("grid", {})
    out_dir = pathlib.Path(spec.get("out_dir", "runs/depth/sweep"))
    out_dir.mkdir(parents=True, exist_ok=True)
    bin_dir = pathlib.Path(args.bin_dir)
    if generic:
        # 生产器自己选卡（parts-train / parts-3d 都没有 --gpu-index），不做跨扫描器租约。
        pool = [0]
    else:
        pool = detect_gpu_pool(bin_dir / "parts-depth-gpu.exe")
        if args.gpus > 0:
            pool = pool[: args.gpus] or list(range(args.gpus))
    jobs = max(
        1,
        args.jobs
        or int(spec.get("jobs", 0))
        or (1 if generic else max(2, len(pool))),
    )
    slots = slot_gpus(jobs, pool)
    verify_on = spec.get("verify", True)
    mdl_tpl = spec.get("model", "{out}/{tag}")

    def model_for(cfg, tag):
        if not generic:
            return out_dir / f"{tag}.dcn"
        t = dict(cfg)
        t.update({"tag": tag, "out": str(out_dir)})
        return pathlib.Path(render_template(mdl_tpl, t))

    configs = build_configs(base, grid)
    grid_keys = sorted(grid)
    data_arg = ",".join(data)
    hold = ",".join(holdout)
    sha = spec_sha(spec)
    queue_path = out_dir / "sweep.queue.json"
    prev = {} if args.fresh else load_queue(queue_path, sha)
    out(
        f"共 {len(configs)} 个配置；数据 {len(data)} 目录；并发 {jobs}；卡池 {pool}（槽 {slots}）；"
        f"重试 {args.retries}；{'fresh' if args.fresh else '续跑'}；输出 {out_dir}"
    )

    if args.dry_run:
        for i, cfg in enumerate(configs):
            tag = tag_of(cfg, grid_keys, base)
            done = is_complete(model_for(cfg, tag), cfg, verify_on)
            if generic:
                t = dict(cfg)
                t.update(
                    {
                        "tag": tag,
                        "out": str(out_dir),
                        "data": ",".join(data),
                        "holdout": ",".join(holdout),
                        "gpu": slots[i % len(slots)],
                    }
                )
                t["model"] = render_template(mdl_tpl, t)
                prod = [spec["producer"]["bin"]] + [
                    render_template(a, t) for a in spec["producer"].get("argv", [])
                ]
                print(f"  [{'skip' if done and not args.fresh else 'run '}] {' '.join(prod)}")
                if verify_on:
                    ver = [spec["verifier"]["bin"]] + [
                        render_template(a, t) for a in spec["verifier"].get("argv", [])
                    ]
                    print(f"        verify: {' '.join(ver)}")
                continue
            cmd = " ".join([
                str(bin_dir / "parts-depth-gpu.exe"), "cnn-train", "--data", f"<{len(data)} dirs>",
                "--holdout", hold, "--out", str(out_dir / f"{tag}.dcn"),
                "--gpu-index", str(slots[i % len(slots)]), *cli_map(cfg),
            ])
            print(f"  [{'skip' if done and not args.fresh else 'run '}] {cmd}")
        return

    if generic:
        ctx = {
            "out_dir": out_dir,
            "data": ",".join(data),
            "holdout": ",".join(holdout),
            "producer_bin": resolve_bin(spec["producer"]["bin"], bin_dir),
            "producer_argv": spec["producer"].get("argv", []),
            "verify_bin": resolve_bin(spec["verifier"]["bin"], bin_dir),
            "verify_argv": spec["verifier"].get("argv", []),
            "model_tpl": mdl_tpl,
            "verify_record_tpl": spec.get("verify_record", "{model}.verify.json"),
            "verify": verify_on,
            "timeout": args.timeout,
            "retries": args.retries,
        }
    else:
        ctx = {
            "out_dir": out_dir, "data_arg": data_arg, "hold": hold,
            "train_bin": bin_dir / "parts-depth-gpu.exe",
            "verify_bin": bin_dir / "parts-depth-verify.exe",
            "verify": verify_on, "timeout": args.timeout, "retries": args.retries,
        }
    for b in ((ctx["producer_bin"], ctx["verify_bin"]) if generic
              else (ctx["train_bin"], ctx["verify_bin"])):
        if not b.exists():
            sys.exit(f"找不到 {b}（先 cargo build --release）")

    states = dict(prev)
    for pid in stale_pids(states):
        if kill_pid(pid):
            out(f"清理上次残留的训练进程 pid={pid}")
    pending = []
    rows = []
    for cfg in configs:
        tag = tag_of(cfg, grid_keys, base)
        model = model_for(cfg, tag)
        if not args.fresh and is_complete(model, cfg, verify_on):
            row = cached_row(tag, cfg, model, verify_on, prev.get(tag))
            rows.append(row)
            states[tag] = {"state": "cached", "metrics": row.get("metrics"), "gpu": row.get("gpu")}
            out(f"[{tag}] 已完成，跳过")
        else:
            pending.append((cfg, tag))

    t0 = time.time()
    tokens = queue.Queue()
    for g in slots:
        tokens.put(g)

    def mark_running(tag, gpu, pid):
        with QUEUE_LOCK:
            states[tag] = {"state": "running", "gpu": gpu, "pid": pid}
            save_queue(queue_path, sha, states)

    # 模板模式的生产器自己选卡 ⇒ 租约没有意义（租了也拦不住它用同一张卡）。
    lease_on = (not args.no_lease) and not generic
    owner = f"{out_dir.name}:{sha[:8]}:{os.getpid()}"

    def task(cfg, tag):
        t_wait = time.time()
        if lease_on:
            g = lease_acquire(pool, owner, args.lease_wait, args.lease_ttl)
            if g is None:
                out(f"[{tag}] 等卡超时（{args.lease_wait}s），跳过")
                return {"tag": tag, "config": cfg, "status": "lease-timeout"}
            waited = time.time() - t_wait
            if waited > 0.5:
                out(f"[{tag}] 排队 {waited:.0f}s 拿到 GPU {g}")
            stop = lease_start(g, owner, args.lease_ttl)
        else:
            g = tokens.get()
            stop = None
        try:
            ctx["on_start"] = lambda pid: mark_running(tag, g, pid)
            row = (run_one_generic if generic else run_one)(cfg, tag, g, ctx)
            with QUEUE_LOCK:
                states[tag] = {
                    "state": row["status"],
                    "gpu": g,
                    "train_attempts": row.get("train_attempts"),
                    "verify_attempts": row.get("verify_attempts"),
                    "metrics": row.get("metrics"),
                }
                save_queue(queue_path, sha, states)
            return row
        finally:
            if lease_on:
                lease_stop(g, owner, stop)
            else:
                tokens.put(g)

    if pending:
        with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as ex:
            futs = [ex.submit(task, cfg, tag) for cfg, tag in pending]
            for fut in concurrent.futures.as_completed(futs):
                rows.append(fut.result())
    wall = time.time() - t0
    order = {tag_of(c, grid_keys, base): i for i, c in enumerate(configs)}
    rows.sort(key=lambda r: order.get(r["tag"], 0))

    rule = spec.get("promote")
    promoted, rejected = [], []
    for r in rows:
        ok, why = evaluate_promotion(r, rule)
        r["promoted"] = ok
        r["promote_reason"] = why
        (promoted if ok else rejected).append({"tag": r["tag"], "why": why})

    summary = out_dir / "sweep.json"
    summary.write_text(
        json.dumps(
            {"spec": spec, "jobs": jobs, "retries": args.retries, "wall_secs": round(wall, 1),
             "rows": rows},
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    (out_dir / "promoted.json").write_text(
        json.dumps(
            {"rule": rule, "promoted": [p["tag"] for p in promoted], "rejected": rejected},
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )
    ok = sum(1 for r in rows if r["status"] in ("verified", "trained"))
    cached = sum(1 for r in rows if r.get("cached"))
    out(
        f"\n完成 {ok}/{len(rows)}（跳过 {cached}，实跑 {len(pending)}）；墙钟 {wall:.0f}s；"
        f"晋级 {len(promoted)} 条；汇总 {summary}"
    )
    for p in promoted:
        out(f"  晋级 {p['tag']}")

    cat = spec.get("catalogue")
    if cat:
        if generic:
            # 模板模式：目录器无参重建**全部**族表（默认路径），不猜是哪一族。
            subprocess.run([sys.executable, "scripts/depth_catalogue.py"], check=False)
            out("目录已重建（全部族表）")
        else:
            # 目录覆盖整棵结果树（默认 runs/depth），不是只扫本次 out_dir
            root = spec.get("catalogue_root", "runs/depth")
            subprocess.run(
                [sys.executable, "scripts/depth_catalogue.py", "--dir", root, "--out", cat],
                check=False,
            )
            out(f"目录已重建 {cat}（root={root}）")

    if any(r["status"] in ("train-failed", "verify-failed") for r in rows):
        sys.exit(1)


if __name__ == "__main__":
    main()
