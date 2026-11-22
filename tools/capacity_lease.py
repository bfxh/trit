#!/usr/bin/env python3
"""capacity-lease: 本机容量租约 + 跨项目协调（不是 cgroup，也不是分布式锁）。

    python tools/capacity_lease.py probe --json
    python tools/capacity_lease.py reserve --project trit --agent main \
        --workers 4 --memory-mb 2048 --ttl 900 --purpose "subagent dispatch"
    python tools/capacity_lease.py status --json
    python tools/capacity_lease.py contract --id cap-...
    python tools/capacity_lease.py release --id cap-...
    python tools/capacity_lease.py --self-test

它解决什么：
  1. 高并发不是“CL/CI 绿了就直接上”。先看这台机器现在还剩多少 CPU/内存；
  2. 多项目共用一台机器时，租约是共享的：A 项目占住的核/内存，B 项目不能
     再按“整机空闲”去算；
  3. 租约带 TTL 和 deadline，派发器把 deadline 写进子智能体的 prompt：
     到点必须收工/交接，不许静默重试或临时加并发。

边界（诚实）：
  * 容量是本机探针的估算，不是 cgroup/Job Object 的硬隔离；租约靠调用方遵守，
    `subagent_dispatch --capacity enforce` 会在调用 provider 前拒绝，但绕过它直接
    调 provider 仍可超。
  * 只覆盖本机；不跨机、不做共识、不防恶意伪造（能写租约目录的人也能改租约）。
  * CPU 容量按逻辑核预留 1 核给系统，再扣当前 1 分钟 load；内存按可用量预留
    最多 1 GiB。Windows 没有 loadavg 时 load1=None，只按逻辑核预留。
  * 过期租约会被清理；时钟回拨/长时间休眠可能让 deadline 失真。
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import secrets
import socket
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone

TOOLS = os.path.dirname(os.path.abspath(__file__))
PY = sys.executable
LEASE_VERSION = 1
LEASE_ID_RX = re.compile(r"^cap-[A-Za-z0-9._-]{6,80}$")
LABEL_RX = re.compile(r"^[^\x00-\x1f\x7f/\\]{1,64}$")
DEFAULT_TTL = 900
MIN_TTL = 30
MAX_TTL = 86400
DEFAULT_MEMORY_MB = 1024
MEMORY_RESERVE_MB = 1024
LOCK_TIMEOUT = 5.0


class CapacityError(RuntimeError):
    pass


def now_epoch():
    return int(time.time())


def iso_utc(epoch):
    return datetime.fromtimestamp(int(epoch), tz=timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _env_int(name, default=None):
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        return int(raw)
    except ValueError:
        return default


def _env_float(name, default=None):
    raw = os.environ.get(name)
    if raw is None or raw == "":
        return default
    try:
        return float(raw)
    except ValueError:
        return default


def default_state_dir():
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.path.join(os.path.expanduser("~"), "AppData", "Local")
        return os.path.join(base, "trit", "capacity")
    base = os.environ.get("XDG_STATE_HOME") or os.path.join(os.path.expanduser("~"), ".local", "state")
    return os.path.join(base, "trit", "capacity")


def resolve_state_dir(explicit=""):
    raw = explicit or os.environ.get("TRIT_CAPACITY_DIR") or default_state_dir()
    return os.path.abspath(os.path.expanduser(raw))


def _lease_dir(state):
    return os.path.join(state, "leases")


def _lock_path(state):
    return os.path.join(state, "broker.lock")


def _windows_memory_mb():
    import ctypes
    from ctypes import wintypes

    class MEMORYSTATUSEX(ctypes.Structure):
        _fields_ = [
            ("dwLength", wintypes.DWORD),
            ("dwMemoryLoad", wintypes.DWORD),
            ("ullTotalPhys", ctypes.c_ulonglong),
            ("ullAvailPhys", ctypes.c_ulonglong),
            ("ullTotalPageFile", ctypes.c_ulonglong),
            ("ullAvailPageFile", ctypes.c_ulonglong),
            ("ullTotalVirtual", ctypes.c_ulonglong),
            ("ullAvailVirtual", ctypes.c_ulonglong),
            ("ullAvailExtendedVirtual", ctypes.c_ulonglong),
        ]

    status = MEMORYSTATUSEX()
    status.dwLength = ctypes.sizeof(status)
    if not ctypes.windll.kernel32.GlobalMemoryStatusEx(ctypes.byref(status)):
        raise CapacityError("GlobalMemoryStatusEx failed")
    return status.ullTotalPhys // (1024 * 1024), status.ullAvailPhys // (1024 * 1024)


def _linux_memory_mb():
    total = available = None
    try:
        with open("/proc/meminfo", "r", encoding="ascii", errors="replace") as fh:
            for line in fh:
                if line.startswith("MemTotal:"):
                    total = int(line.split()[1]) // 1024
                elif line.startswith("MemAvailable:"):
                    available = int(line.split()[1]) // 1024
    except OSError as exc:
        raise CapacityError(f"cannot read /proc/meminfo: {exc}") from exc
    if total is None:
        raise CapacityError("/proc/meminfo has no MemTotal")
    return total, available if available is not None else total


def _mac_memory_mb():
    try:
        total = (os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")) // (1024 * 1024)
    except (OSError, ValueError):
        total = 0
    available = None
    try:
        import psutil  # type: ignore
        available = int(psutil.virtual_memory().available) // (1024 * 1024)
        total = total or int(psutil.virtual_memory().total) // (1024 * 1024)
    except Exception:
        pass
    if total <= 0:
        raise CapacityError("cannot determine physical memory")
    return total, available if available is not None else max(1, int(total * 0.7))


def probe_memory_mb():
    if os.name == "nt":
        return _windows_memory_mb()
    if sys.platform == "linux":
        return _linux_memory_mb()
    if sys.platform == "darwin":
        return _mac_memory_mb()
    try:
        import psutil  # type: ignore
        vm = psutil.virtual_memory()
        return int(vm.total) // (1024 * 1024), int(vm.available) // (1024 * 1024)
    except Exception as exc:
        raise CapacityError(f"cannot probe memory: {exc}") from exc


def probe_load1(cores):
    override = _env_float("TRIT_CAPACITY_ASSUME_LOAD1")
    if override is not None:
        return max(0.0, override), "override"
    try:
        return max(0.0, float(os.getloadavg()[0])), "getloadavg"
    except (AttributeError, OSError):
        pass
    try:
        import psutil  # type: ignore
        load = float(psutil.getloadavg()[0])
        return max(0.0, load), "psutil"
    except Exception:
        return None, "unknown"


def probe(cpu_override=None, memory_override_mb=None):
    cores = max(1, int(os.cpu_count() or 1))
    assume_cpu = cpu_override if cpu_override is not None else _env_int("TRIT_CAPACITY_ASSUME_CPU")
    assume_mem = memory_override_mb if memory_override_mb is not None else _env_int(
        "TRIT_CAPACITY_ASSUME_MEM_MB")
    if assume_cpu is not None:
        cores = max(1, int(assume_cpu))
    total_mb, available_mb = probe_memory_mb()
    if assume_mem is not None:
        total_mb = max(1, int(assume_mem))
        available_mb = max(1, int(assume_mem))
    load1, load_source = probe_load1(cores)
    cpu_workers = max(0, cores - 1 - int(math.floor(load1 or 0.0))) if load1 is not None else max(0, cores - 1)
    memory_reserve = min(MEMORY_RESERVE_MB, max(256, available_mb // 8))
    memory_capacity = max(0, available_mb - memory_reserve)
    state = resolve_state_dir()
    try:
        import shutil
        disk_path = state
        while disk_path and not os.path.exists(disk_path):
            parent = os.path.dirname(disk_path)
            if parent == disk_path:
                break
            disk_path = parent
        disk_free_mb = shutil.disk_usage(disk_path).free // (1024 * 1024)
    except OSError:
        disk_free_mb = 0
    return {
        "host": socket.gethostname(),
        "platform": sys.platform,
        "pid": os.getpid(),
        "probed_at": now_epoch(),
        "cpu": {
            "logical": cores,
            "load1": None if load1 is None else round(load1, 3),
            "load_source": load_source,
            "capacity_workers": cpu_workers,
            "reserved_for_os": 1,
        },
        "memory": {
            "total_mb": total_mb,
            "available_mb": available_mb,
            "reserve_mb": memory_reserve,
            "capacity_mb": memory_capacity,
        },
        "disk": {"free_mb": disk_free_mb},
        "capacity": {"cpu_workers": cpu_workers, "memory_mb": memory_capacity},
        "assumptions": [
            "cpu: one logical core reserved for the OS; subtract floor(load1) when loadavg exists",
            "memory: keep up to 1 GiB (or 1/8 available) for the OS/other work",
            "not a cgroup/Job Object; leases are advisory until the caller enforces them",
        ],
    }


class BrokerLock:
    """Cross-process broker lock. Windows byte-range locks are per-process, so the
    cross-project concurrency test uses subprocesses, not threads."""

    def __init__(self, state, timeout=LOCK_TIMEOUT):
        self.path = _lock_path(state)
        self.timeout = timeout
        self.fd = None

    def __enter__(self):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        self.fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o600)
        if os.fstat(self.fd).st_size == 0:
            os.write(self.fd, b"\0")
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                os.lseek(self.fd, 0, os.SEEK_SET)
                if os.name == "nt":
                    import msvcrt
                    msvcrt.locking(self.fd, msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(self.fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return self
            except OSError:
                if time.monotonic() >= deadline:
                    os.close(self.fd)
                    self.fd = None
                    raise CapacityError(f"capacity broker lock timeout: {self.path}")
                time.sleep(0.05)

    def __exit__(self, exc_type, exc, tb):
        if self.fd is None:
            return False
        try:
            os.lseek(self.fd, 0, os.SEEK_SET)
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(self.fd, msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(self.fd, fcntl.LOCK_UN)
        finally:
            os.close(self.fd)
            self.fd = None
        return False


def _clean_label(value, what, required=False):
    if value is None:
        value = ""
    value = str(value).strip()
    if not value:
        if required:
            raise CapacityError(f"{what} must be non-empty")
        return ""
    if len(value) > 64 or not LABEL_RX.match(value):
        raise CapacityError(f"{what} must be 1..64 chars without controls or path separators")
    return value


def _clean_purpose(value):
    value = str(value or "").strip()
    if len(value) > 200:
        raise CapacityError("purpose must be <= 200 chars")
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
        raise CapacityError("purpose contains control characters")
    return value


def _validate_lease(doc):
    if not isinstance(doc, dict):
        raise CapacityError("lease is not an object")
    if doc.get("version") != LEASE_VERSION:
        raise CapacityError("lease version mismatch")
    lid = doc.get("id")
    if not isinstance(lid, str) or not LEASE_ID_RX.match(lid):
        raise CapacityError("bad lease id")
    for key in ("project", "host", "created_at", "expires_at", "heartbeat_at", "pid"):
        if key not in doc:
            raise CapacityError(f"lease missing {key}")
    for key in ("cpu_workers", "memory_mb"):
        value = doc.get("resources", {}).get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise CapacityError(f"lease resources.{key} must be a non-negative integer")
    if not isinstance(doc["expires_at"], int) or not isinstance(doc["created_at"], int):
        raise CapacityError("lease timestamps must be integers")
    return doc


def _read_lease(path):
    with open(path, "r", encoding="utf-8-sig") as fh:
        return _validate_lease(json.load(fh))


def _scan(state, now):
    os.makedirs(_lease_dir(state), exist_ok=True)
    active, invalid, expired = [], [], []
    for name in sorted(os.listdir(_lease_dir(state))):
        if not name.endswith(".json"):
            continue
        path = os.path.join(_lease_dir(state), name)
        try:
            lease = _read_lease(path)
        except Exception as exc:
            invalid.append({"path": path, "error": str(exc)[:160]})
            continue
        if lease["expires_at"] <= now:
            expired.append(lease["id"])
            try:
                os.remove(path)
            except OSError:
                pass
            continue
        active.append(lease)
    active.sort(key=lambda x: (x["created_at"], x["id"]))
    return active, invalid, expired


def _write_lease(state, lease):
    path = os.path.join(_lease_dir(state), lease["id"] + ".json")
    tmp = path + f".tmp.{os.getpid()}.{secrets.token_hex(4)}"
    with open(tmp, "w", encoding="utf-8", newline="\n") as fh:
        json.dump(lease, fh, ensure_ascii=True, indent=2, sort_keys=True)
        fh.write("\n")
    os.replace(tmp, path)
    return path


def _usage(active):
    cpu = sum(x["resources"]["cpu_workers"] for x in active)
    mem = sum(x["resources"]["memory_mb"] for x in active)
    return cpu, mem


def _lease_id():
    return "cap-" + secrets.token_hex(12)


def _result(ok, status, **extra):
    base = {"ok": bool(ok), "status": status}
    base.update(extra)
    return base


def reserve(project, agent="", workers=1, memory_mb=DEFAULT_MEMORY_MB, ttl=DEFAULT_TTL,
            purpose="", state_dir="", now=None, probe_data=None, dry_run=False):
    project = _clean_label(project, "project", required=True)
    agent = _clean_label(agent, "agent")
    purpose = _clean_purpose(purpose)
    for name, value, minimum in (("workers", workers, 1), ("memory_mb", memory_mb, 1),
                                 ("ttl", ttl, MIN_TTL)):
        if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
            return _result(False, "bad-request", reason=f"{name} must be an integer >= {minimum}")
    if ttl > MAX_TTL:
        return _result(False, "bad-request", reason=f"ttl must be <= {MAX_TTL}")
    state = resolve_state_dir(state_dir)
    now = now_epoch() if now is None else int(now)
    with BrokerLock(state):
        active, invalid, expired = _scan(state, now)
        if invalid:
            return _result(False, "capacity-unreadable",
                           reason="one or more lease files are unreadable; refusing rather than guessing",
                           invalid=invalid, expired=expired)
        snapshot = probe_data or probe()
        used_cpu, used_mem = _usage(active)
        free_cpu = max(0, snapshot["capacity"]["cpu_workers"] - used_cpu)
        free_mem = max(0, snapshot["capacity"]["memory_mb"] - used_mem)
        if workers > free_cpu or memory_mb > free_mem:
            return _result(False, "capacity-refused",
                           reason=(f"requested {workers} workers / {memory_mb} MB, "
                                   f"available {free_cpu} workers / {free_mem} MB"),
                           requested={"workers": workers, "memory_mb": memory_mb},
                           available={"cpu_workers": free_cpu, "memory_mb": free_mem},
                           used={"cpu_workers": used_cpu, "memory_mb": used_mem},
                           capacity=snapshot, leases=active, expired=expired)
        lease = {
            "version": LEASE_VERSION,
            "id": _lease_id(),
            "project": project,
            "agent": agent,
            "purpose": purpose,
            "host": socket.gethostname(),
            "pid": os.getpid(),
            "cwd": os.getcwd(),
            "created_at": now,
            "heartbeat_at": now,
            "expires_at": now + ttl,
            "state": "active",
            "resources": {"cpu_workers": workers, "memory_mb": memory_mb},
        }
        if not dry_run:
            _write_lease(state, lease)
        return _result(True, "granted" if not dry_run else "dry-run", lease=lease,
                       requested={"workers": workers, "memory_mb": memory_mb},
                       available_before={"cpu_workers": free_cpu, "memory_mb": free_mem},
                       available_after={"cpu_workers": free_cpu - workers,
                                        "memory_mb": free_mem - memory_mb},
                       used={"cpu_workers": used_cpu, "memory_mb": used_mem},
                       capacity=snapshot, leases=active, expired=expired,
                       state_dir=state, dry_run=bool(dry_run))


def _find(state, lease_id):
    lease_id = str(lease_id or "").strip()
    if not LEASE_ID_RX.match(lease_id):
        raise CapacityError("bad lease id")
    path = os.path.join(_lease_dir(state), lease_id + ".json")
    if not os.path.isfile(path):
        return None, path
    return _read_lease(path), path


def release(lease_id, state_dir="", owner="", now=None):
    state = resolve_state_dir(state_dir)
    now = now_epoch() if now is None else int(now)
    with BrokerLock(state):
        lease, path = _find(state, lease_id)
        if lease is None:
            return _result(False, "not-found", lease_id=lease_id, state_dir=state)
        if owner:
            owner = str(owner).strip()
            if owner not in (lease.get("project"), f"{lease.get('project')}:{lease.get('agent')}"):
                return _result(False, "owner-mismatch", lease=lease,
                               reason=f"lease belongs to {lease.get('project')}:{lease.get('agent')}")
        try:
            os.remove(path)
        except OSError as exc:
            return _result(False, "release-failed", lease=lease, error=str(exc))
        return _result(True, "released", lease=lease, state_dir=state)


def renew(lease_id, ttl=DEFAULT_TTL, state_dir="", now=None):
    if isinstance(ttl, bool) or not isinstance(ttl, int) or ttl < MIN_TTL or ttl > MAX_TTL:
        return _result(False, "bad-request", reason=f"ttl must be {MIN_TTL}..{MAX_TTL}")
    state = resolve_state_dir(state_dir)
    now = now_epoch() if now is None else int(now)
    with BrokerLock(state):
        lease, path = _find(state, lease_id)
        if lease is None:
            return _result(False, "not-found", lease_id=lease_id)
        if lease["expires_at"] <= now:
            return _result(False, "expired", lease=lease)
        lease["heartbeat_at"] = now
        lease["expires_at"] = now + ttl
        _write_lease(state, lease)
        return _result(True, "renewed", lease=lease, state_dir=state)


def status(state_dir="", now=None):
    state = resolve_state_dir(state_dir)
    now = now_epoch() if now is None else int(now)
    with BrokerLock(state):
        active, invalid, expired = _scan(state, now)
        snapshot = probe()
        used_cpu, used_mem = _usage(active)
        by_project = {}
        for lease in active:
            key = lease["project"]
            row = by_project.setdefault(key, {"cpu_workers": 0, "memory_mb": 0, "leases": 0})
            row["cpu_workers"] += lease["resources"]["cpu_workers"]
            row["memory_mb"] += lease["resources"]["memory_mb"]
            row["leases"] += 1
        return _result(not invalid, "ok" if not invalid else "capacity-unreadable",
                       state_dir=state, now=now,
                       capacity=snapshot,
                       used={"cpu_workers": used_cpu, "memory_mb": used_mem},
                       free={"cpu_workers": max(0, snapshot["capacity"]["cpu_workers"] - used_cpu),
                             "memory_mb": max(0, snapshot["capacity"]["memory_mb"] - used_mem)},
                       leases=active, by_project=by_project,
                       invalid=invalid, expired=expired)


def contract(lease, now=None):
    if isinstance(lease, str):
        raise CapacityError("contract expects a lease object, not an id; call find first")
    now = now_epoch() if now is None else int(now)
    if lease["expires_at"] <= now:
        raise CapacityError("lease expired; renew before handing it to a subagent")
    resources = lease["resources"]
    return (
        "CAPACITY CONTRACT (enforced by the parent before this task starts)\n"
        f"- lease: {lease['id']} on {lease['host']}\n"
        f"- project / agent: {lease['project']} / {lease.get('agent') or '-'}\n"
        f"- workers: at most {resources['cpu_workers']}; do not spawn extra threads or processes\n"
        f"- memory: at most {resources['memory_mb']} MB\n"
        f"- deadline: {iso_utc(lease['expires_at'])} (UTC); finish or hand off before it\n"
        "- shared host: other projects may hold leases; stay inside this slice\n"
        "- if blocked or unable to finish, stop and report; do not silently retry or raise concurrency\n"
        "- do not write outside the task's declared output paths"
    )


def _print(doc, as_json=False):
    if as_json:
        print(json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True))
        return
    status_name = doc.get("status", "?")
    if status_name in ("granted", "dry-run"):
        lease = doc["lease"]
        print(f"capacity: {status_name} {lease['id']} workers={lease['resources']['cpu_workers']} "
              f"memory={lease['resources']['memory_mb']}MB deadline={iso_utc(lease['expires_at'])}")
    elif status_name == "ok":
        cap = doc["capacity"]["capacity"]
        print(f"capacity: host={doc['capacity']['host']} cpu={doc['used']['cpu_workers']}/"
              f"{cap['cpu_workers']} mem={doc['used']['memory_mb']}/{cap['memory_mb']}MB "
              f"leases={len(doc['leases'])}")
        for lease in doc["leases"]:
            print(f"  {lease['id']} {lease['project']}:{lease.get('agent') or '-'} "
                  f"{lease['resources']['cpu_workers']}w/{lease['resources']['memory_mb']}MB "
                  f"until {iso_utc(lease['expires_at'])}")
    elif status_name == "refused" or status_name == "capacity-refused":
        print(f"capacity: refused -- {doc.get('reason', '')}")
    elif status_name in ("released", "renewed"):
        print(f"capacity: {status_name} {doc['lease']['id']} until "
              f"{iso_utc(doc['lease']['expires_at'])}")
    else:
        print(f"capacity: {status_name} -- {doc.get('reason', doc.get('error', ''))}")


def _build_parser():
    p = argparse.ArgumentParser(description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--self-test", action="store_true")
    sub = p.add_subparsers(dest="cmd")
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--state-dir", default="")
    common.add_argument("--json", action="store_true")

    probe_p = sub.add_parser("probe", parents=[common])
    probe_p.set_defaults(func="probe")
    status_p = sub.add_parser("status", parents=[common])
    status_p.set_defaults(func="status")
    reserve_p = sub.add_parser("reserve", parents=[common])
    reserve_p.set_defaults(func="reserve")
    reserve_p.add_argument("--project", required=True)
    reserve_p.add_argument("--agent", default="")
    reserve_p.add_argument("--workers", type=int, default=1)
    reserve_p.add_argument("--memory-mb", type=int, default=DEFAULT_MEMORY_MB)
    reserve_p.add_argument("--ttl", type=int, default=DEFAULT_TTL)
    reserve_p.add_argument("--purpose", default="")
    reserve_p.add_argument("--dry-run", action="store_true")
    release_p = sub.add_parser("release", parents=[common])
    release_p.set_defaults(func="release")
    release_p.add_argument("--id", required=True)
    release_p.add_argument("--owner", default="")
    renew_p = sub.add_parser("renew", parents=[common])
    renew_p.set_defaults(func="renew")
    renew_p.add_argument("--id", required=True)
    renew_p.add_argument("--ttl", type=int, default=DEFAULT_TTL)
    contract_p = sub.add_parser("contract", parents=[common])
    contract_p.set_defaults(func="contract")
    contract_p.add_argument("--id", required=True)
    return p


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    args = _build_parser().parse_args(argv)
    if getattr(args, "self_test", False):
        return self_test()
    if not getattr(args, "cmd", None):
        print("capacity-lease: choose probe/status/reserve/release/renew/contract or --self-test",
              file=sys.stderr)
        return 2
    try:
        if args.cmd == "probe":
            doc = probe()
            doc["status"] = "ok"
            _print(doc, args.json)
            return 0
        if args.cmd == "status":
            doc = status(args.state_dir)
            _print(doc, args.json)
            return 0 if doc["ok"] else 2
        if args.cmd == "reserve":
            doc = reserve(args.project, args.agent, args.workers, args.memory_mb,
                          args.ttl, args.purpose, args.state_dir, dry_run=args.dry_run)
            _print(doc, args.json)
            return 0 if doc["ok"] else 2
        if args.cmd == "release":
            doc = release(args.id, args.state_dir, args.owner)
            _print(doc, args.json)
            return 0 if doc["ok"] else 2
        if args.cmd == "renew":
            doc = renew(args.id, args.ttl, args.state_dir)
            _print(doc, args.json)
            return 0 if doc["ok"] else 2
        if args.cmd == "contract":
            state = resolve_state_dir(args.state_dir)
            lease, _ = _find(state, args.id)
            if lease is None:
                doc = _result(False, "not-found", lease_id=args.id)
                _print(doc, args.json)
                return 2
            print(contract(lease))
            return 0
    except CapacityError as exc:
        print(f"capacity: error -- {exc}", file=sys.stderr)
        return 2
    return 2


def _check(checks, name, cond, detail=""):
    checks.append((name, bool(cond), detail))


def self_test():
    checks = []
    tmp = tempfile.mkdtemp(prefix="capacity-self-")
    state = os.path.join(tmp, "state")
    try:
        p = probe(cpu_override=8, memory_override_mb=8192)
        _check(checks, "probe has capacity", p["capacity"]["cpu_workers"] == 7
               and p["capacity"]["memory_mb"] > 0, str(p["capacity"]))
        r1 = reserve("project-a", "main", 3, 1024, 60, "first", state,
                     now=1000, probe_data=p)
        _check(checks, "reserve grants", r1["ok"] and r1["status"] == "granted"
               and r1["available_after"]["cpu_workers"] == 4, str(r1))
        r2 = reserve("project-b", "worker", 5, 1024, 60, "second", state,
                     now=1000, probe_data=p)
        _check(checks, "cross-project over-capacity refused",
               not r2["ok"] and r2["status"] == "capacity-refused", str(r2))
        s1 = status(state, now=1000)
        _check(checks, "status lists both projects", s1["ok"]
               and s1["by_project"]["project-a"]["cpu_workers"] == 3
               and len(s1["leases"]) == 1, str(s1["by_project"]))
        text = contract(r1["lease"], now=1000)
        _check(checks, "contract carries budget and deadline",
               "at most 3" in text and "memory: at most 1024 MB" in text
               and "deadline:" in text and "do not spawn extra threads" in text)
        rel = release(r1["lease"]["id"], state, owner="project-a")
        _check(checks, "release works", rel["ok"] and rel["status"] == "released")
        r3 = reserve("project-c", "worker", 1, 512, 60, "after-release", state,
                     now=1000, probe_data=p)
        _check(checks, "capacity returned after release", r3["ok"], str(r3))
        release(r3["lease"]["id"], state)
        expired = reserve("project-d", "worker", 1, 512, 30, "short", state,
                          now=1001, probe_data=p)
        _check(checks, "short lease granted", expired["ok"], str(expired))
        s2 = status(state, now=1031)
        _check(checks, "expired lease is pruned", not s2["leases"]
               and expired["lease"]["id"] in s2["expired"], str(s2["expired"]))
        with open(os.path.join(_lease_dir(state), "bad.json"), "w", encoding="utf-8") as fh:
            fh.write("{not json")
        r4 = reserve("project-e", "worker", 1, 512, 60, "bad", state,
                     now=1003, probe_data=p)
        _check(checks, "unreadable lease fails closed",
               not r4["ok"] and r4["status"] == "capacity-unreadable", str(r4))
        os.remove(os.path.join(_lease_dir(state), "bad.json"))

        # Cross-process lock: 8 processes each ask 1 worker / 512 MB against a
        # fake 4-worker / 2560 MB host; exactly 4 may win.
        cstate = os.path.join(tmp, "concurrent")
        env = os.environ.copy()
        env.update({"TRIT_CAPACITY_DIR": cstate,
                    "TRIT_CAPACITY_ASSUME_CPU": "5",
                    "TRIT_CAPACITY_ASSUME_MEM_MB": "2560",
                    "TRIT_CAPACITY_ASSUME_LOAD1": "0"})
        procs = [subprocess.Popen([PY, os.path.abspath(__file__), "reserve",
                                   "--project", f"p{i}", "--workers", "1",
                                   "--memory-mb", "512", "--ttl", "60", "--json"],
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  text=True, encoding="utf-8", env=env)
                 for i in range(8)]
        outs = []
        for proc in procs:
            out, err = proc.communicate(timeout=30)
            outs.append((proc.returncode, out, err))
        wins = sum(1 for rc, _, _ in outs if rc == 0)
        _check(checks, "cross-process lease honors capacity", wins == 4,
               f"wins={wins} results={[o[0] for o in outs]}")

        # Dispatch integration: --capacity enforce acquires before any provider call
        # and injects the contract into the prompt.
        import subagent_dispatch as sd
        root = os.path.join(tmp, "repo")
        os.makedirs(root)
        cfg = {"version": 1, "models": [{
            "id": "echo", "provider": "command",
            "command": [PY, "-c", "import sys; print(sys.stdin.read())"],
            "roles": ["docs"], "priority": 1, "cost": "free", "context": 1000,
            "capabilities": ["text"]}], "routes": {"docs": ["echo"], "default": ["echo"]}}
        cfg_path = os.path.join(tmp, "models.json")
        with open(cfg_path, "w", encoding="utf-8") as fh:
            json.dump(cfg, fh)
        os.environ["TRIT_CAPACITY_DIR"] = os.path.join(tmp, "dispatch")
        os.environ["TRIT_CAPACITY_ASSUME_CPU"] = "5"
        os.environ["TRIT_CAPACITY_ASSUME_MEM_MB"] = "2560"
        os.environ["TRIT_CAPACITY_ASSUME_LOAD1"] = "0"
        plan = {"version": 1, "tasks": [{"id": "doc", "role": "docs",
                                         "prompt": "hello", "out": "out/doc.txt"}]}
        args = sd.make_parser().parse_args([
            "--plan", "x", "--root", root, "--artifact-dir", os.path.join(root, "art"),
            "--parallel", "4", "--capacity", "enforce",
            "--capacity-dir", os.environ["TRIT_CAPACITY_DIR"]])
        doc, rc = sd.dispatch(root, plan, cfg, cfg_path, args)
        payload = open(os.path.join(root, "out/doc.txt"), encoding="utf-8").read()
        _check(checks, "dispatch enforces capacity and injects contract",
               rc == 0 and doc["ok"] and doc["capacity"]["lease"]
               and "CAPACITY CONTRACT" in payload
               and "at most 4" in payload, str(doc.get("capacity")))
        s3 = status(os.environ["TRIT_CAPACITY_DIR"], now=now_epoch())
        _check(checks, "dispatch releases lease", not s3["leases"], str(s3["leases"]))

        # Refuse before provider call when the slice is already taken.
        hold = reserve("holder", "test", 4, 2048, 300,
                       "hold all", os.environ["TRIT_CAPACITY_DIR"])
        args2 = sd.make_parser().parse_args([
            "--plan", "x", "--root", root, "--artifact-dir", os.path.join(root, "art2"),
            "--parallel", "4", "--capacity", "enforce",
            "--capacity-dir", os.environ["TRIT_CAPACITY_DIR"]])
        plan2 = {"version": 1, "tasks": [{"id": "doc2", "role": "docs",
                                          "prompt": "hello", "out": "out/doc2.txt"}]}
        doc2, rc2 = sd.dispatch(root, plan2, cfg, cfg_path, args2)
        _check(checks, "dispatch refuses before call when capacity is held",
               rc2 == 2 and doc2["status"] == "capacity-refused"
               and not os.path.exists(os.path.join(root, "out/doc2.txt")), str(doc2.get("status")))
        release(hold["lease"]["id"], os.environ["TRIT_CAPACITY_DIR"])
    except Exception as exc:
        _check(checks, "self-test did not crash", False, f"{type(exc).__name__}: {exc}")
    finally:
        import shutil
        shutil.rmtree(tmp, ignore_errors=True)
    passed = sum(1 for _, ok, _ in checks if ok)
    for name, ok, detail in checks:
        print(("ok   " if ok else "FAIL ") + name + (("  [" + detail + "]") if detail and not ok else ""))
    print(f"capacity-lease self-test: {passed}/{len(checks)} passed")
    return 0 if passed == len(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
