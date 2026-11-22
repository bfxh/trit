#!/usr/bin/env python3
"""capacity-lease: 本机容量租约 + 跨项目协调（不是 cgroup，也不是分布式锁）。

    python tools/capacity_lease.py probe --json
    python tools/capacity_lease.py reserve --project trit --agent main \
        --workers 4 --memory-mb 2048 --gpu 1 --disk-mb 4096 --io-units 4 \
        --ttl 900 --purpose "subagent dispatch"
    python tools/capacity_lease.py status --json
    python tools/capacity_lease.py contract --id cap-...
    python tools/capacity_lease.py renew --id cap-... --ttl 600
    python tools/capacity_lease.py release --id cap-...
    python tools/capacity_lease.py --self-test

它解决什么：
  1. 高并发不是“CL/CI 绿了就直接上”。先看这台机器现在还剩多少 CPU/内存/GPU/磁盘/IO；
  2. 多项目共用一台机器时，租约是共享的：A 项目占住的资源，B 项目不能
     再按“整机空闲”去算；
  3. 租约带可续租 TTL 和不可续租的硬 deadline，派发器把 deadline 写进子智能体的
     prompt：到点必须收工/交接，不许静默重试或临时加并发；
  4. 长任务用 `LeaseHeartbeat` 续租，但 heartbeat 不会把硬 deadline 往后推。

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
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
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
DEFAULT_IO_UNITS = 16
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


def probe_gpu():
    """GPU 不是可移植探针：`nvidia-smi` 能列出几张就报几张，否则报 0/unknown。

    capacity 是准入估算，不是驱动层的硬隔离；请求 GPU > 0 时如果探针给 0，
    reserve 会 fail-closed 拒绝，而不是假设“应该有”。
    """
    override = _env_int("TRIT_CAPACITY_ASSUME_GPU")
    if override is not None:
        return max(0, int(override)), "override"
    exe = shutil.which("nvidia-smi")
    if not exe:
        return 0, "unavailable"
    try:
        result = subprocess.run(
            [exe, "--query-gpu=name", "--format=csv,noheader"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, encoding="utf-8", errors="replace", timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return 0, f"probe-failed:{type(exc).__name__}"
    if result.returncode != 0:
        return 0, f"probe-failed:exit={result.returncode}"
    names = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    return len(names), "nvidia-smi"


def probe_io_units():
    override = _env_int("TRIT_CAPACITY_ASSUME_IO")
    if override is not None:
        return max(0, int(override)), "override"
    return DEFAULT_IO_UNITS, "default-advisory"


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


def probe(cpu_override=None, memory_override_mb=None, gpu_override=None,
          disk_override_mb=None, io_override=None, state_dir=""):
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
    gpu_count, gpu_source = probe_gpu()
    if gpu_override is not None:
        gpu_count, gpu_source = max(0, int(gpu_override)), "override"
    io_units, io_source = probe_io_units()
    if io_override is not None:
        io_units, io_source = max(0, int(io_override)), "override"
    state = resolve_state_dir(state_dir)
    try:
        disk_path = state
        while disk_path and not os.path.exists(disk_path):
            parent = os.path.dirname(disk_path)
            if parent == disk_path:
                break
            disk_path = parent
        disk_free_mb = shutil.disk_usage(disk_path).free // (1024 * 1024)
    except OSError:
        disk_free_mb = 0
    if disk_override_mb is not None:
        disk_free_mb = max(0, int(disk_override_mb))
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
        "gpu": {"count": gpu_count, "source": gpu_source},
        "disk": {"free_mb": disk_free_mb},
        "io": {"capacity_units": io_units, "source": io_source},
        "capacity": {
            "cpu_workers": cpu_workers,
            "memory_mb": memory_capacity,
            "gpu": gpu_count,
            "disk_mb": disk_free_mb,
            "io_units": io_units,
        },
        "assumptions": [
            "cpu: one logical core reserved for the OS; subtract floor(load1) when loadavg exists",
            "memory: keep up to 1 GiB (or 1/8 available) for the OS/other work",
            "gpu: nvidia-smi count, 0 when the probe is unavailable; a positive request is refused then",
            "disk: free space of the lease state filesystem, not a scratch-volume reservation",
            "io: advisory slots, not a syscall/bandwidth measurement",
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
    if not isinstance(doc["expires_at"], int) or not isinstance(doc["created_at"], int):
        raise CapacityError("lease timestamps must be integers")
    # 老租约没有 deadline_at / gpu / disk / io：补默认值而不是让 reader 崩。
    if not isinstance(doc.get("deadline_at"), int):
        doc["deadline_at"] = doc["expires_at"]
    resources = doc.setdefault("resources", {})
    for key in ("cpu_workers", "memory_mb"):
        value = resources.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise CapacityError(f"lease resources.{key} must be a non-negative integer")
    for key in ("gpu", "disk_mb", "io_units"):
        if key not in resources:
            resources[key] = 0
        value = resources[key]
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise CapacityError(f"lease resources.{key} must be a non-negative integer")
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


def _usage_resources(active):
    used = {"cpu_workers": 0, "memory_mb": 0, "gpu": 0, "disk_mb": 0, "io_units": 0}
    for lease in active:
        resources = lease["resources"]
        for key in used:
            used[key] += int(resources.get(key) or 0)
    return used


def _capacity_resources(snapshot):
    cap = snapshot["capacity"]
    return {
        "cpu_workers": int(cap["cpu_workers"]),
        "memory_mb": int(cap["memory_mb"]),
        "gpu": int(cap.get("gpu") or 0),
        "disk_mb": int(cap["disk_mb"] if "disk_mb" in cap
                        else snapshot.get("disk", {}).get("free_mb") or 0),
        "io_units": int(cap.get("io_units") or 0),
    }


def _free_resources(snapshot, used):
    cap = _capacity_resources(snapshot)
    return {key: max(0, cap[key] - used.get(key, 0)) for key in cap}


def _lease_id():
    return "cap-" + secrets.token_hex(12)


def _result(ok, status, **extra):
    base = {"ok": bool(ok), "status": status}
    base.update(extra)
    return base


def reserve(project, agent="", workers=1, memory_mb=DEFAULT_MEMORY_MB, ttl=DEFAULT_TTL,
            purpose="", state_dir="", now=None, probe_data=None, dry_run=False,
            gpu=0, disk_mb=0, io_units=0):
    project = _clean_label(project, "project", required=True)
    agent = _clean_label(agent, "agent")
    purpose = _clean_purpose(purpose)
    for name, value, minimum in (("workers", workers, 1), ("memory_mb", memory_mb, 1),
                                 ("ttl", ttl, MIN_TTL), ("gpu", gpu, 0),
                                 ("disk_mb", disk_mb, 0), ("io_units", io_units, 0)):
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
        snapshot = probe_data or probe(state_dir=state)
        used = _usage_resources(active)
        free = _free_resources(snapshot, used)
        request = {"cpu_workers": workers, "memory_mb": memory_mb,
                   "gpu": gpu, "disk_mb": disk_mb, "io_units": io_units}
        over = [key for key, value in request.items() if value > free[key]]
        if over:
            return _result(False, "capacity-refused",
                           reason=(f"requested {request}, available {free}; over: {', '.join(over)}"),
                           requested=request, available=free, used=used,
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
            "deadline_at": now + ttl,
            "state": "active",
            "resources": request,
        }
        if not dry_run:
            _write_lease(state, lease)
        after = {key: free[key] - request[key] for key in free}
        return _result(True, "granted" if not dry_run else "dry-run", lease=lease,
                       requested=request, available_before=free, available_after=after,
                       used=used, capacity=snapshot, leases=active, expired=expired,
                       state_dir=state, dry_run=bool(dry_run))


def _find(state, lease_id):
    lease_id = str(lease_id or "").strip()
    if not LEASE_ID_RX.match(lease_id):
        raise CapacityError("bad lease id")
    path = os.path.join(_lease_dir(state), lease_id + ".json")
    if not os.path.isfile(path):
        return None, path
    return _read_lease(path), path


def load_lease(lease_id, state_dir=""):
    """Public read-only lookup for tools that attach to an existing lease."""
    state = resolve_state_dir(state_dir)
    with BrokerLock(state):
        lease, _ = _find(state, lease_id)
        if lease is None:
            return _result(False, "not-found", lease_id=lease_id, state_dir=state)
        return _result(True, "active", lease=lease, state_dir=state)


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
        deadline = int(lease.get("deadline_at") or lease["expires_at"])
        if lease["expires_at"] <= now:
            return _result(False, "expired", lease=lease)
        if now >= deadline:
            return _result(False, "deadline-exceeded", lease=lease,
                           reason="hard deadline reached; do not extend the lease")
        lease["heartbeat_at"] = now
        # TTL 可以续，但硬 deadline 不能续：heartbeat 只防“未完成就被别人抢走”，
        # 不把长时间运行偷偷变成无限预算。
        lease["expires_at"] = min(now + ttl, deadline)
        lease["deadline_at"] = deadline
        _write_lease(state, lease)
        return _result(True, "renewed", lease=lease, state_dir=state)


class LeaseHeartbeat:
    """后台续租线程：只在 deadline 前续 TTL，不延长硬 deadline。

    daemon=True，父进程退出不会把续租线程留成孤儿；stop() 必须 join，避免
    释放租约后线程又把同一 id 复活（release 后 renew 会 not-found）。
    """

    def __init__(self, lease_id, ttl=DEFAULT_TTL, state_dir="", interval=None):
        self.lease_id = lease_id
        self.ttl = int(ttl)
        self.state_dir = state_dir
        self.interval = max(1, int(interval or max(5, self.ttl // 3)))
        self.renewals = 0
        self.last = None
        self.error = None
        self._stop = threading.Event()
        self._thread = None

    def _run(self):
        while not self._stop.wait(self.interval):
            try:
                result = renew(self.lease_id, self.ttl, self.state_dir)
            except Exception as exc:
                self.error = {"status": "heartbeat-error", "error": str(exc)}
                return
            if not result.get("ok"):
                self.error = result
                return
            self.renewals += 1
            self.last = result

    def start(self):
        if self._thread is not None:
            return self
        self._thread = threading.Thread(target=self._run, name="capacity-heartbeat",
                                        daemon=True)
        self._thread.start()
        return self

    def stop(self):
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=max(1, self.interval + 1))
        return {
            "status": "ok" if self.error is None else "failed",
            "interval_s": self.interval,
            "renewals": self.renewals,
            "error": self.error,
            "last": self.last,
        }


def status(state_dir="", now=None):
    state = resolve_state_dir(state_dir)
    now = now_epoch() if now is None else int(now)
    with BrokerLock(state):
        active, invalid, expired = _scan(state, now)
        snapshot = probe(state_dir=state)
        used = _usage_resources(active)
        free = _free_resources(snapshot, used)
        by_project = {}
        for lease in active:
            key = lease["project"]
            row = by_project.setdefault(key, {"cpu_workers": 0, "memory_mb": 0,
                                              "gpu": 0, "disk_mb": 0, "io_units": 0,
                                              "leases": 0})
            for resource in ("cpu_workers", "memory_mb", "gpu", "disk_mb", "io_units"):
                row[resource] += int(lease["resources"].get(resource) or 0)
            row["leases"] += 1
        return _result(not invalid, "ok" if not invalid else "capacity-unreadable",
                       state_dir=state, now=now, capacity=snapshot,
                       used=used, free=free,
                       leases=active, by_project=by_project,
                       invalid=invalid, expired=expired)


def contract(lease, now=None):
    if isinstance(lease, str):
        raise CapacityError("contract expects a lease object, not an id; call find first")
    now = now_epoch() if now is None else int(now)
    deadline = int(lease.get("deadline_at") or lease["expires_at"])
    if lease["expires_at"] <= now:
        raise CapacityError("lease expired; renew before handing it to a subagent")
    if deadline <= now:
        raise CapacityError("lease deadline reached; do not hand it to a subagent")
    resources = lease["resources"]
    return (
        "CAPACITY CONTRACT (enforced by the parent before this task starts)\n"
        f"- lease: {lease['id']} on {lease['host']}\n"
        f"- project / agent: {lease['project']} / {lease.get('agent') or '-'}\n"
        f"- workers: at most {resources['cpu_workers']}; do not spawn extra threads or processes\n"
        f"- memory: at most {resources['memory_mb']} MB\n"
        f"- gpu: at most {resources.get('gpu', 0)} device slot(s)\n"
        f"- disk: at most {resources.get('disk_mb', 0)} MB of scratch/free-space budget\n"
        f"- io: at most {resources.get('io_units', 0)} advisory IO slot(s)\n"
        f"- deadline: {iso_utc(deadline)} (UTC); finish or hand off before it\n"
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
              f"memory={lease['resources']['memory_mb']}MB gpu={lease['resources'].get('gpu', 0)} "
              f"deadline={iso_utc(lease.get('deadline_at') or lease['expires_at'])}")
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
    reserve_p.add_argument("--gpu", type=int, default=0)
    reserve_p.add_argument("--disk-mb", type=int, default=0)
    reserve_p.add_argument("--io-units", type=int, default=0)
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
            doc = probe(state_dir=args.state_dir)
            doc["status"] = "ok"
            _print(doc, args.json)
            return 0
        if args.cmd == "status":
            doc = status(args.state_dir)
            _print(doc, args.json)
            return 0 if doc["ok"] else 2
        if args.cmd == "reserve":
            doc = reserve(args.project, args.agent, args.workers, args.memory_mb,
                          args.ttl, args.purpose, args.state_dir, dry_run=args.dry_run,
                          gpu=args.gpu, disk_mb=args.disk_mb, io_units=args.io_units)
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
        p = probe(cpu_override=8, memory_override_mb=8192, gpu_override=2,
                  disk_override_mb=4096, io_override=8)
        _check(checks, "probe has capacity", p["capacity"]["cpu_workers"] == 7
               and p["capacity"]["memory_mb"] > 0, str(p["capacity"]))
        _check(checks, "probe has gpu/disk/io dimensions",
               p["capacity"]["gpu"] == 2 and p["capacity"]["disk_mb"] == 4096
               and p["capacity"]["io_units"] == 8, str(p["capacity"]))
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
               and "gpu: at most" in text and "disk: at most" in text
               and "io: at most" in text and "deadline:" in text
               and "do not spawn extra threads" in text)
        res = reserve("project-res", "worker", 1, 512, 60, "resources", state,
                      now=1000, probe_data=p, gpu=1, disk_mb=1024, io_units=2)
        _check(checks, "gpu/disk/io are leased", res["ok"]
               and res["lease"]["resources"]["gpu"] == 1
               and res["lease"]["resources"]["disk_mb"] == 1024
               and res["lease"]["resources"]["io_units"] == 2, str(res))
        res_over = reserve("project-res2", "worker", 1, 512, 60, "gpu-over", state,
                           now=1000, probe_data=p, gpu=2)
        _check(checks, "gpu over-request refused", not res_over["ok"]
               and res_over["status"] == "capacity-refused", str(res_over))
        release(res["lease"]["id"], state)
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

        # Heartbeat: renews the TTL while the task runs, but never moves the
        # hard deadline.
        hstate = os.path.join(tmp, "heartbeat")
        hnow = now_epoch()
        hres = reserve("project-hb", "worker", 1, 512, 30, "heartbeat", hstate,
                       now=hnow, probe_data=p)
        deadline = hres["lease"]["deadline_at"]
        hb = LeaseHeartbeat(hres["lease"]["id"], 30, hstate, interval=1).start()
        time.sleep(1.2)
        hb_doc = hb.stop()
        loaded = load_lease(hres["lease"]["id"], hstate)
        _check(checks, "heartbeat renews without moving deadline",
               hb_doc["renewals"] >= 1 and loaded["ok"]
               and loaded["lease"]["deadline_at"] == deadline
               and loaded["lease"]["expires_at"] <= deadline, str(hb_doc))
        hb_again = hb.stop()
        _check(checks, "heartbeat stop is idempotent", hb_again["status"] == "ok"
               and hb_again["renewals"] == hb_doc["renewals"], str(hb_again))
        release(hres["lease"]["id"], hstate)

        # A TTL shorter than the hard deadline can expire first; contract must
        # refuse the expired lease even while the deadline is still in the future.
        sstate = os.path.join(tmp, "short-ttl")
        short = reserve("project-short", "worker", 1, 512, 60, "short-ttl", sstate,
                        now=1000, probe_data=p)
        renewed = renew(short["lease"]["id"], 30, sstate, now=1020)
        try:
            contract(renewed["lease"], now=1055)
            short_expired = False
        except CapacityError:
            short_expired = True
        _check(checks, "contract refuses expired TTL before deadline", short_expired)

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
    # 只打印失败项：checks.sh 截断到窗口尾部时，18 行 ok 会把真正失败的两项挤出日志。
    for name, ok, detail in checks:
        if ok:
            continue
        print("FAIL " + name + (("  [" + detail + "]") if detail else ""))
    print(f"capacity-lease self-test: {passed}/{len(checks)} passed")
    return 0 if passed == len(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
